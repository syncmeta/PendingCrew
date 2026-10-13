# Claude true-PTY/ACP reuse checkpoint — source research only

## Fixed upstream input

On 2026-10-14 the GitHub commits API resolved `moabualruz/claude-code-cli-acp` main to `ef7a01436fb4b552f66dabb7bac0e9d2b46e31c0`. Primary files were fetched at this exact ref; no upstream executable, install command, Claude account or model was run.

- [Pinned source](https://github.com/moabualruz/claude-code-cli-acp/tree/ef7a01436fb4b552f66dabb7bac0e9d2b46e31c0)
- [Package configuration](https://github.com/moabualruz/claude-code-cli-acp/blob/ef7a01436fb4b552f66dabb7bac0e9d2b46e31c0/Cargo.toml): package 0.1.1, Rust requirement 1.95, portable-pty 0.9.0, ACP crates pinned 0.11.1 with the protocol unstable feature enabled.
- [License](https://github.com/moabualruz/claude-code-cli-acp/blob/ef7a01436fb4b552f66dabb7bac0e9d2b46e31c0/LICENSE) and [NOTICE](https://github.com/moabualruz/claude-code-cli-acp/blob/ef7a01436fb4b552f66dabb7bac0e9d2b46e31c0/NOTICE.md): the project declares Apache-2.0 and has a separate attribution notice. Any actual copied/distributed portion must retain relevant provenance and notices; no code or packaging artwork has been copied in this checkpoint.

## Program implication

Keep Codex native app-server and the current core toolchain as the critical path. The candidate is not a drop-in workspace dependency merely because it speaks ACP: its Rust requirement differs from core 1.94, and its pinned unstable protocol feature needs explicit capability/version boundaries. An isolated helper or selective reuse remains a candidate decision for M4, with its own reproducible build record and maintained notice files; do not upgrade the whole workspace just to try it.

Earlier PTY source inspection established a genuine interactive launch candidate, not completed compatibility. Dynamic resize, terminal takeover, native approvals, transcript integrity, process-group stop and durable resume must still pass the full true-PTY gate. ACP tool-terminal support alone does not prove the main agent runs interactively. Common communication/permissions/work/recovery remain owned by crewd; no second provider-specific team ledger.

This checkpoint removes a future provenance/packaging unknown while M2/M3 implementation continues. It does not change the present delivery order or claim Claude integration, license compatibility of every dependency, runtime acceptance or model-cost savings.
