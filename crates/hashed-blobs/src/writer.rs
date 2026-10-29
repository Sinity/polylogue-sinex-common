//! The single append owner and its dedicated durability thread.
use crate::{Error, Extent, Hash, Prepared, Result, codec};
use std::{
    collections::{HashMap, VecDeque},
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupMetrics {
    pub materials: u64,
    pub appended_materials: u64,
    pub deduplicated_materials: u64,
    pub appended_bytes: u64,
    pub syncs: u64,
}
struct SyncRequest {
    reply: mpsc::Sender<std::io::Result<()>>,
}
struct GroupRecord {
    id: u64,
    end: u64,
    hashes: Vec<Hash>,
}
pub struct SegmentWriter {
    file: File,
    segment: String,
    end: u64,
    committed_end: u64,
    next: u64,
    pending: HashMap<Hash, Extent>,
    groups: VecDeque<GroupRecord>,
    sync: Option<mpsc::Sender<SyncRequest>>,
    worker: Option<thread::JoinHandle<()>>,
    failed: Arc<AtomicBool>,
    identity: u64,
}
pub struct PendingGroup {
    group: SyncedGroup,
    reply: mpsc::Receiver<std::io::Result<()>>,
}
#[derive(Debug)]
pub struct SyncedGroup {
    pub id: u64,
    pub extents: Vec<Extent>,
    pub end: u64,
    pub metrics: GroupMetrics,
    identity: u64,
}
impl PendingGroup {
    /// Wait for the group's single data sync, executed on the writer's worker.
    pub fn wait(self) -> Result<SyncedGroup> {
        self.reply
            .recv()
            .map_err(|_| Error::Refused("segment sync worker stopped".into()))??;
        Ok(self.group)
    }
}
impl SegmentWriter {
    /// The product's committed end and recorded punches are authoritative.
    /// A missing segment with committed bytes is an integrity failure.
    pub fn open(
        root: impl AsRef<Path>,
        segment: impl Into<String>,
        committed_end: u64,
        punches: &[Extent],
        mode: u32,
    ) -> Result<Self> {
        let segment = segment.into();
        codec::segment_name(&segment)?;
        let path = root.as_ref().join(&segment);
        if let Some(parent) = path.parent() {
            create_directories(parent)?;
        }
        let existed = path.exists();
        if !existed && committed_end != 0 {
            return Err(Error::Corrupt {
                segment,
                offset: 0,
                len: committed_end,
                what: "committed segment is missing".into(),
            });
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(mode)
            .open(&path)?;
        // An advisory exclusive lock names the actual file, and is inherited
        // by the durability worker's duplicate descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let length = file.metadata()?.len();
        if length < committed_end {
            return Err(Error::Corrupt {
                segment,
                offset: length,
                len: committed_end - length,
                what: "segment is shorter than its committed end".into(),
            });
        }
        if length != committed_end {
            file.set_len(committed_end)?;
            #[cfg(test)]
            boundary("truncate");
            file.sync_data()?;
        }
        if !existed {
            File::open(path.parent().expect("segment parent"))?.sync_all()?;
        }
        for extent in punches {
            validate_punch(&segment, committed_end, extent)?;
            punch(&file, extent)?;
        }
        if !punches.is_empty() {
            file.sync_data()?;
        }
        let worker_file = file.try_clone()?;
        let (tx, rx) = mpsc::channel::<SyncRequest>();
        let failed = Arc::new(AtomicBool::new(false));
        let failure = Arc::clone(&failed);
        let worker = thread::Builder::new()
            .name("segment-sync".into())
            .spawn(move || {
                for request in rx {
                    let result = if failure.load(Ordering::Acquire) {
                        Err(std::io::Error::other(
                            "segment writer stopped after an earlier failure",
                        ))
                    } else {
                        worker_file.sync_data()
                    };
                    if result.is_err() {
                        failure.store(true, Ordering::Release);
                    }
                    #[cfg(test)]
                    if result.is_ok() {
                        boundary("sync");
                    }
                    let _ = request.reply.send(result);
                }
            })?;
        let mut identity = [0; 8];
        use chacha20poly1305::aead::rand_core::RngCore;
        chacha20poly1305::aead::OsRng.fill_bytes(&mut identity);
        Ok(Self {
            file,
            segment,
            end: committed_end,
            committed_end,
            next: 1,
            pending: HashMap::new(),
            groups: VecDeque::new(),
            sync: Some(tx),
            worker: Some(worker),
            failed,
            identity: u64::from_le_bytes(identity),
        })
    }
    /// Append in input order. Committed and uncommitted dedup both return the
    /// existing descriptor. An append or sync failure requires ordinary reopen.
    pub fn append_group(
        &mut self,
        prepared: Vec<Prepared>,
        mut lookup: impl FnMut(&Hash) -> Result<Option<Extent>>,
    ) -> Result<PendingGroup> {
        if self.failed.load(Ordering::Acquire) {
            return Err(Error::Refused(
                "segment writer failed; reopen at the committed end".into(),
            ));
        }
        let result = self.append(prepared, &mut lookup);
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
        }
        result
    }
    fn append(
        &mut self,
        prepared: Vec<Prepared>,
        lookup: &mut impl FnMut(&Hash) -> Result<Option<Extent>>,
    ) -> Result<PendingGroup> {
        let mut extents = Vec::with_capacity(prepared.len());
        let mut hashes = Vec::new();
        let mut metrics = GroupMetrics {
            materials: prepared.len() as u64,
            syncs: 1,
            ..GroupMetrics::default()
        };
        for mut item in prepared {
            let known = if let Some(extent) = self.pending.get(&item.hash) {
                Some(extent.clone())
            } else {
                lookup(&item.hash)?
            };
            if let Some(extent) = known {
                if extent.hash != item.hash
                    || extent.decoded_len != item.decoded_len
                    || extent.kind != item.kind
                {
                    return Err(Error::Refused(
                        "hash index returned a different blob descriptor".into(),
                    ));
                }
                metrics.deduplicated_materials += 1;
                extents.push(extent);
                continue;
            }
            let offset = self.end;
            let end = offset
                .checked_add(item.stored_len)
                .ok_or_else(|| Error::Refused("segment length overflows".into()))?;
            self.file.seek(SeekFrom::Start(offset))?;
            item.file.rewind()?;
            let mut n = 0u64;
            let mut hasher = blake3::Hasher::new();
            let mut buffer = [0; 65536];
            loop {
                let got = item.file.read(&mut buffer)?;
                if got == 0 {
                    break;
                }
                n = n
                    .checked_add(got as u64)
                    .ok_or_else(|| Error::Refused("prepared length overflows".into()))?;
                if n > item.stored_len {
                    return Err(Error::Refused(
                        "prepared encoding changed before append".into(),
                    ));
                }
                hasher.update(&buffer[..got]);
                self.file.write_all(&buffer[..got])?;
            }
            if n != item.stored_len || Hash(*hasher.finalize().as_bytes()) != item.encoded_hash {
                return Err(Error::Refused(
                    "prepared encoding changed before append".into(),
                ));
            }
            let extent = Extent {
                segment: self.segment.clone(),
                offset,
                len: item.stored_len,
                decoded_len: item.decoded_len,
                hash: item.hash,
                kind: item.kind,
                index_hash: item.index_hash,
                encoded_hash: item.encoded_hash,
            };
            self.end = end;
            metrics.appended_materials += 1;
            metrics.appended_bytes += n;
            hashes.push(extent.hash);
            self.pending.insert(extent.hash, extent.clone());
            extents.push(extent);
        }
        // Flush user-space buffers before asking the dedicated thread to sync.
        self.file.flush()?;
        #[cfg(test)]
        boundary("append");
        let (reply, rx) = mpsc::channel();
        self.sync
            .as_ref()
            .expect("live sync thread")
            .send(SyncRequest { reply })
            .map_err(|_| Error::Refused("segment sync worker stopped".into()))?;
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| Error::Refused("group sequence overflows".into()))?;
        self.groups.push_back(GroupRecord {
            id,
            end: self.end,
            hashes,
        });
        Ok(PendingGroup {
            group: SyncedGroup {
                id,
                extents,
                end: self.end,
                metrics,
                identity: self.identity,
            },
            reply: rx,
        })
    }
    /// Called only after the product commits this group's extents and end in
    /// the same transaction as their referring rows. Acknowledgments are FIFO.
    pub fn acknowledge(&mut self, group: &SyncedGroup) -> Result<()> {
        let first = self
            .groups
            .front()
            .ok_or_else(|| Error::Refused("no group awaits commit acknowledgment".into()))?;
        if group.identity != self.identity || first.id != group.id || first.end != group.end {
            return Err(Error::Refused(
                "commit acknowledgment is from another writer or out of order".into(),
            ));
        }
        let first = self.groups.pop_front().expect("front checked");
        self.committed_end = first.end;
        for hash in first.hashes {
            self.pending.remove(&hash);
        }
        Ok(())
    }
    pub fn committed_end(&self) -> u64 {
        self.committed_end
    }
    /// The product records these punches durably before calling this method.
    /// Linux releases whole blocks and zeroes the partial end blocks.
    pub fn punch(&mut self, extents: &[Extent]) -> Result<()> {
        for extent in extents {
            validate_punch(&self.segment, self.committed_end, extent)?;
        }
        for extent in extents {
            punch(&self.file, extent)?;
        }
        if !extents.is_empty() {
            self.file.sync_data()?;
        }
        Ok(())
    }
}
impl Drop for SegmentWriter {
    fn drop(&mut self) {
        self.sync.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn validate_punch(segment: &str, end: u64, extent: &Extent) -> Result<()> {
    if extent.segment != segment
        || extent
            .offset
            .checked_add(extent.len)
            .is_none_or(|until| until > end)
    {
        return Err(Error::Refused(
            "punch lies outside the committed segment".into(),
        ));
    }
    Ok(())
}
fn punch(file: &File, extent: &Extent) -> Result<()> {
    let offset = i64::try_from(extent.offset)
        .map_err(|_| Error::Refused("punch offset exceeds filesystem address space".into()))?;
    let len = i64::try_from(extent.len)
        .map_err(|_| Error::Refused("punch length exceeds filesystem address space".into()))?;
    if len == 0 {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        if unsafe {
            libc::fallocate(
                file.as_raw_fd(),
                libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE,
                offset,
                len,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (file, offset, len);
        Err(Error::Refused(
            "hole punching is unavailable on this platform".into(),
        ))
    }
}

fn create_directories(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        create_directories(parent)?;
    }
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()?;
    Ok(())
}
#[cfg(test)]
fn boundary(point: &str) {
    if std::env::var("HASHED_BLOBS_CRASH_BOUNDARY").is_ok_and(|value| value == point) {
        unsafe { libc::raise(libc::SIGSTOP) };
        unreachable!("parent kills the stopped crash worker");
    }
}
#[cfg(test)]
mod tests;
