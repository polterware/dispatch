//! Git worktrees for write runs: create, inspect, merge, clean.

mod git;
mod merge;
mod ops;

pub use git::{
    git_exit, parse_git_version, set_supports_merge_tree_override, supports_merge_tree,
    version_supports_merge_tree,
};
pub use merge::{
    DEFAULT_LOCK_TIMEOUT, MergeLock, MergeTreeResult, lock_merge, lock_merge_with_timeout,
    lock_timeout, merge_tree, set_lock_timeout, uncommitted_overlaps,
};
pub use ops::{
    NumStat, branch_name, commit, create, current_branch, dirty, exclude, forget, has_commits,
    head, main_root, merge, nothing_to_merge, num_stat, recover, rel, remove, remove_folder, root,
    worktree_path,
};
