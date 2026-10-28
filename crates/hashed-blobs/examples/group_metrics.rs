//! Hardware-independent amplification and one-second commit cadence report.
use hashed_blobs::{Preparation, SegmentWriter};
use std::{
    io::Cursor,
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let count = std::env::args()
        .nth(1)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(10_000usize);
    let dir = tempfile::tempdir()?;
    let preparation = Preparation::new(dir.path().join("scratch"))?;
    let mut writer = SegmentWriter::open(dir.path(), "segment.zst", 0, &[], 0o600)?;
    let started = Instant::now();
    let mut prepared = Vec::new();
    let mut input_bytes = 0u64;
    let mut groups = 0u64;
    let mut written = 0u64;
    let mut next = started + Duration::from_secs(1);
    for number in 0..count {
        let bytes = format!(
            "{{\"id\":{number},\"role\":\"assistant\",\"content\":\"{}\"}}\n",
            "synthetic content with repeated structure ".repeat(40)
        );
        input_bytes += bytes.len() as u64;
        prepared.push(preparation.plain(Cursor::new(bytes), 3)?);
        if Instant::now() >= next {
            let group = writer
                .append_group(std::mem::take(&mut prepared), |_| Ok(None))?
                .wait()?;
            written += group.metrics.appended_bytes;
            writer.acknowledge(&group)?;
            groups += 1;
            next = Instant::now() + Duration::from_secs(1);
        }
    }
    if !prepared.is_empty() {
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
        let group = writer.append_group(prepared, |_| Ok(None))?.wait()?;
        written += group.metrics.appended_bytes;
        writer.acknowledge(&group)?;
        groups += 1;
    }
    let elapsed = started.elapsed().as_secs_f64();
    println!("materials={count} groups={groups} input_bytes={input_bytes} appended_bytes={written} bytes_per_input_byte={:.6} materials_per_second={:.2} commit_interval_seconds=1 elapsed_seconds={elapsed:.3}",written as f64/input_bytes as f64,count as f64/elapsed);
    Ok(())
}
