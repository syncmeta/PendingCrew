# Candidate source pairing — isolated validator acceptance

## Result and scope

QA commit `d5a3c283` extends the real artifact-check entry point to require a complete input manifest and build record. The old checker accepted a signed synthetic package with a fabricated SHA and only the three historic metadata files. Preserved RED: `/private/tmp/pcw-provenance-missing-proof-red-01.log`. The new checker rejects this incomplete package.

The corrected structural validator accepts a complete synthetic control and rejects twelve source, manifest, pre-sign binary, packaged binary, log and prebuilt-input mismatches. It also accepts different GUI/backend commit labels only when the complete Rust snapshot inputs match and the prebuilt binary and retained witness match. GREEN: `/private/tmp/pcw-provenance-checker-green-02.log`; retained fixture evidence `/private/tmp/pcw-provenance-qa.C5pNE7`.

These are small synthetic validator controls. Their mock input files and mock build logs deliberately test the evidence format; they do not prove that the real PendingCrew app was built, establish an independently trusted attestation, or replace actual compiler execution. Existing signature/hash checks remain separate from input pairing. No actual app, model, installation or publication was started.

## Integration ownership and next executable gate

The client author owns the immutable committed-input builder, full GUI input registration, candidate source/version labels and actual compiler invocation. QA owns the read-only artifact checker and independent pipeline validation. The backend author owns the only Rust target/compiler window. They coordinate directly on the witness format; root owns final consistency and delivery acceptance.

After the complete M2 module is frozen, capture the immutable complete Rust input snapshot and canonical digest before the next necessary actual build. Preserve the command, exit status, original compiler output, pre-sign binary and digest. Use that exact binary in the GUI package when the package Rust inputs match. Do not infer provenance from a later HEAD or substitute only a declared source ref. A matching recorded prebuilt input permits reuse rather than a second cold release build; it does not make the synthetic validator result a real build result.

M3 remains incomplete until the real builder/checker chain, consistent packaged runtime, visible-window flow and authorized actual use/migration gates pass. M1/M2 acceptance and later M4–M6 remain distinct program gates.

## Configuration gate still pending

Exact source inputs alone do not prove toolchain, target, profile or feature equivalence. The actual builder and prebuilt witness must retain the real configuration and compare it with the chosen candidate configuration. An explicitly selected debug local candidate is permitted; an all-features/testkit binary must not silently become a default-feature release artifact. Client and QA own this additional structural check and actual execution integration. The accepted `d5a3c283` synthetic controls are not evidence that this configuration gate is already implemented.
