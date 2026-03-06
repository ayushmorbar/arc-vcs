---
name: arc-cas-storage
description: Strict constraints for reading and writing to the arc content-addressable storage (CAS) file system. Use whenever handling disk I/O.
---
# Instructions
1. **Hashing:** ALWAYS use the `blake3` crate. Never use SHA-1 or SHA-256.
2. **Serialization:** ALWAYS use `bincode` for binary serialization of objects. Never use JSON for disk storage.
3. **File Paths:** Objects must be written to `.arc/store/{hash[0:2]}/{hash[2:]}`.
4. **Memory Safety:** When reading the change graph, you must use `memmap2` for zero-copy deserialization to meet the sub-100ms performance budget.