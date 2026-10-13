# Claude ACP native foundation checkpoint

This checkpoint is an in-progress M4 foundation, not a working full ACP provider or a release.

The existing Claude PTY driver now accepts an optional pre-exec native birth gate and bounded terminal observer. The normal path remains unchanged. Birth reports include actual PID, generation and private nonce; exec waits for the exact parent's acknowledgement. Timeout, disconnected parent and wrong-generation acknowledgements cannot execute the synthetic native child. Acknowledged execution has real controlling stdin/stdout/stderr TTY and `/dev/tty`.

ACP JSON control and native binary terminal frames use different codecs. Native capability negotiation requires the explicit extension rather than accepting ACP handshake as evidence of native parity. Request ownership and terminal ordering reject duplicate/old generations. Control and terminal Debug omit prompt/body bytes. Each real PTY read is observed exactly once, including synchronous final drains. Bounded broadcast lag is explicit; it must lead to a coherent requested screen snapshot before resetting sequence baseline. A slow viewer never waits inside PTY reads.

Executed local synthetic evidence:

- `/private/tmp/pcw-M4-birth-gate-02.log`: four real pre-exec/controlling-PTY cases passed.
- `/private/tmp/pcw-M4-terminal-wire-01.log`: eleven birth, wire, capability, ordering, redaction and bounded lag cases passed (handle 65137 exit 0).
- `/private/tmp/pcw-M4-real-terminal-tap-01.log`: actual ANSI/UTF8 PTY output split between async and immediate-drain reads mirrored exactly once, passed.

No paid CLI, personal transcript/auth, installed runtime, full ACP helper, lifecycle recovery, mixed Codex route or independent M4 acceptance is claimed. Production initialize must not advertise these planned capabilities until the full implementation is connected and proven. Next work remains helper/core transport, single-writer enrollment, authenticated Stop-hook completion, confirmed resize, permission fidelity, prompt delivery/recovery deduplication and process-group stop. The Linux M3 delivery failure is being addressed in a separate exact-source tree before resuming this lifecycle.
