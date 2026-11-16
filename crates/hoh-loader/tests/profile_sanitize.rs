//! `app-boot/tests/profile-sanitize.spec.ts` has five filesystem recovery cases.
//! None is a pure reader/composition case: all call rename/write and recovery APIs
//! absent from the binding §2.9. Their exact inventory is in PORT_NOTES.md.
