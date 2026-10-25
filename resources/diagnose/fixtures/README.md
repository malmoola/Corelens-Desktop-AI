# Diagnose fixtures

One folder per platform, `healthy/` and `broken/`, one file per playbook
command id. Every check of that platform's playbook must find nothing in
`healthy/` and fire on `broken/` (a command with no broken file reads the
healthy one); `crates/corelens-diagnose/tests/playbooks.rs` holds that.

These outputs are reconstructed from the vendors' command references and
from the shapes of the ntc-templates test captures vendored under
`resources/templates/tests/` — they are not captures from a device. Names,
serials and addresses are invented (documentation ranges, `FAKE…`
serials). When a capture from a real device replaces one, its folder's
playbook can move from `verified: docs` to `verified: lab`.
