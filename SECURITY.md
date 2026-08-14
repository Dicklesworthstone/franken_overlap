# Security Policy

## Reporting

Please report security-sensitive issues privately to the repository owner rather than opening a public issue with exploit details.

## Threat model

FrankenOverlap may process untrusted text and untrusted `.foidx` artifacts. The parser therefore validates magic/version, flags, reserved bytes, counts, lengths, ordering, document references, positions, frequencies, and EOF before returning an index.

The default Rust core forbids unsafe code. Optional GPU execution is expected to remain inside FrankenTorch’s reviewed Metal boundary.

## Resource limits

Applications should set corpus file-size, posting-list, candidate, verification-band, and dense-work limits appropriate to their environment. A valid but enormous corpus can still consume substantial CPU, memory, and disk.

## Hashes and sketches

Fingerprints and CountSketch scores are not cryptographic authenticity mechanisms. They generate candidates only. Final lexical results are verified against normalized tokens.
