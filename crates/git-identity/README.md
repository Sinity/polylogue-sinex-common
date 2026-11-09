# git-identity

Which Git repository a directory belongs to, read the same way by Polylogue
and Sinex.

Polylogue joins sessions to repositories and Sinex attributes shell commands
to projects by these facts. When each product read them itself they
disagreed: one stored the `origin` URL with its credentials, one took only the
first-parent root commit, and two counted shallow-clone boundaries as roots,
so the products could name different repositories for one directory.

## What `observe(dir, deadline)` returns

An `Identity`:

- `common_dir`: the repository's common directory, the same for every
  worktree of one repository;
- `root_commits`: every commit reachable from `HEAD` with no parent, sorted
  and without repeats, leaving out the boundary commits of a shallow clone;
- `head`: the commit `HEAD` names;
- `branch`: the checked-out branch, none when `HEAD` is detached;
- `remote`: the `origin` URL with any user and password removed.

Two directories belong to the same repository when their root sets share a
commit. A set is used rather than one commit because it stays the same when
histories are merged in either direction; the first-parent root changes with
the direction of a merge. A boundary commit of a shallow clone has parents the
clone did not fetch, so it is not a root, and a shallow clone whose history
reaches no real root has an empty set rather than looking like a different
repository.

Git runs with the repository environment variables removed and with
replacement objects and lazy fetching off, so a caller's `GIT_DIR` or a
partial clone cannot change the answer or reach the network. Output goes to a
temporary file so a large answer never blocks Git, and every query shares the
one deadline. `Unavailable` says why no identity could be read: not a
repository with a commit, Git missing, the deadline passed, or an unreadable
answer.

`without_credentials`, `root_set` and `repository_of` are the pieces callers
that already hold Git's output use.
