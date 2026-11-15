# russh 0.65.1, as Corelens carries it

The crate as published on crates.io (Apache-2.0, LICENSE beside this file,
https://github.com/Eugeny/russh), with one change, marked `CORELENS PATCH`
in the source: the MAC `hmac-sha1-96` (RFC 4253), which russh does not
implement and which older switches are often locked to. It is the existing
HMAC-SHA1 with its tag cut to the first 12 bytes — `TruncatedMacAlgorithm`
in `src/mac/crypto.rs`, registered in `src/mac/mod.rs`. Corelens offers it
last, after every other MAC.

`crates/corelens-discover/tests/ssh_mac_sha1_96.rs` checks it against
OpenSSH's own `sshd` allowing that MAC alone. This copy goes when a russh
release carries the algorithm.
