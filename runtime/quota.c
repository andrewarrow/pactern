/* Explicitly opted-in quota ABI 1. Slot selectors and tokens convey only the
 * authority already granted by the embedding Rust host. No tenant strings,
 * filesystem paths, or ambient service lookup cross this ABI. */
__attribute__((import_module("pactern.quota.v1"),import_name("root")))
U32 pt_quota_root(U32 slot);
__attribute__((import_module("pactern.quota.v1"),import_name("read")))
I64 pt_quota_read(U32 token);
__attribute__((import_module("pactern.quota.v1"),import_name("reserve")))
U32 pt_quota_reserve(U32 token,I64 amount);
__attribute__((import_module("pactern.quota.v1"),import_name("commit")))
void pt_quota_commit(U32 token);
__attribute__((import_module("pactern.quota.v1"),import_name("rollback")))
void pt_quota_rollback(U32 token);
__attribute__((import_module("pactern.quota.v1"),import_name("release")))
void pt_quota_release(U32 token);
