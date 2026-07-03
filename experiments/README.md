# Experiments — the dead-ends that shaped the design

These are throwaway proof-of-concept programs from early bring-up. They're kept because the
**findings** drove every language/runtime decision in the real appliance (`service/irapi/`).
Only the source is here; the compiled binaries were build artifacts and are not committed.

## The one finding that mattered: **no `futex` → no Go**

The Harmony Hub's kernel (Linux 2.6.31, AR9331) is built **without `CONFIG_FUTEX`**. The Go
runtime needs `futex` to bring up its scheduler, so **any Go binary crashes at startup** before
`main()` runs — regardless of version or `GOMAXPROCS`.

| dir | language | result |
|-----|----------|--------|
| `hello/` | Go | Trivial `hello world`. Cross-compiled fine (mips, softfloat, `CGO_ENABLED=0`) but **crashed on the device** at runtime — first proof the futex problem is fatal. |
| `probe/` | Go | A syscall/environment probe meant to enumerate `/dev`, kernel config, etc. Same futex crash — never produced output. |
| `rustprobe/` | Rust | A **single-threaded** static `mips-unknown-linux-musl` binary. **Ran cleanly** on the device — proved Rust (no threads, uncontended std locks only) is viable. Its `Cargo.toml` + `.cargo/config.toml` became the template for `service/irapi/`. |

**Consequence:** the appliance is written in **single-threaded Rust**, `panic=abort`, zero runtime
crates, static musl. See [`service/irapi/`](../service/irapi/) for the real thing and
[`docs/ir-service-buildplan.md`](../docs/ir-service-buildplan.md) for the toolchain recipe.

> Also learned here: **UPX doesn't work on this kernel** (its self-extracting stub `SIGTRAP`s) —
> deploy the native binary, `zip -9` it only for the slow UART upload path.
