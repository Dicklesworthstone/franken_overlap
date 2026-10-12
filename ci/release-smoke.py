#!/usr/bin/env python3
"""Exercise already-built release executables from an unrelated working directory.

Usage: python3 ci/release-smoke.py /absolute/path/to/extracted/binaries [version]
No Cargo, repository fixtures, or external network access is required.
"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    if len(sys.argv) not in (2, 3):
        raise SystemExit(__doc__)
    bindir = Path(sys.argv[1]).resolve(strict=True)
    version = sys.argv[2] if len(sys.argv) == 3 else None
    executables = {name: bindir / name for name in ("fo", "fo-corpus")}
    for binary in executables.values():
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise RuntimeError(f"missing or non-executable release binary: {binary}")

    with tempfile.TemporaryDirectory(prefix="fo-release-smoke-") as temporary:
        work = Path(temporary)
        environment = dict(os.environ, HOME=str(work), SEC_USER_AGENT="")

        def run(name, *arguments, success=True):
            result = subprocess.run(
                [str(executables[name]), *map(str, arguments)],
                cwd=work, env=environment, capture_output=True, text=True, timeout=60,
            )
            if success and result.returncode != 0:
                raise RuntimeError(f"{name} {arguments} failed: {result.stderr}")
            if not success and result.returncode == 0:
                raise RuntimeError(f"{name} {arguments} unexpectedly succeeded")
            return result.stdout

        for name in executables:
            output = run(name, "--version").strip()
            print(output)
            if version and output != f"{name} {version}":
                raise RuntimeError(f"release version mismatch: {output!r}")
            run(name, "--help")

        # Deliberately more than the default 24 matched tokens, with an edited
        # word in the middle. The unrelated document must not be attributed.
        original = (
            "Our manufacturing operations depend on reliable suppliers of raw materials "
            "and components throughout the year. Interruptions in transportation may "
            "delay production and increase operating costs. We maintain alternative "
            "sources of supply and regularly evaluate inventory levels to reduce "
            "these risks while continuing to serve customers in domestic and "
            "international markets with consistent quality and timely delivery."
        )
        corpus = work / "corpus"
        corpus.mkdir()
        (corpus / "filing.txt").write_text(original, encoding="utf-8")
        (corpus / "unrelated.txt").write_text(
            "Astronomers observed distant galaxies through a telescope and recorded "
            "the brightness of stars across the northern sky during winter.",
            encoding="utf-8",
        )
        index = work / "corpus.foidx"
        report = json.loads(run("fo", "index", corpus, "--output", index, "--json"))
        if report["documents"] != 2:
            raise RuntimeError(f"incorrect indexed document count: {report}")
        inspection = json.loads(run("fo", "inspect", index, "--json"))
        if len(inspection["documents"]) != 2:
            raise RuntimeError("installed CLI cannot reopen the written index")
        for specimen in (original, original.replace("reliable suppliers", "dependable suppliers")):
            hits = json.loads(run("fo", "query", index, "--text", specimen, "--json"))
            if not hits or hits[0]["path"] != "filing.txt" or hits[0]["matched_tokens"] < 24:
                raise RuntimeError(f"incorrect edited-passage attribution: {hits}")
            if any(hit["path"] == "unrelated.txt" for hit in hits):
                raise RuntimeError("unrelated document was accepted as a source")

        run("fo", "inspect", work / "missing.foidx", success=False)
        damaged = bytearray(index.read_bytes())
        damaged[len(damaged) // 2] ^= 1
        corrupt = work / "corrupt.foidx"
        corrupt.write_bytes(damaged)
        run("fo", "inspect", corrupt, success=False)

        run("fo-corpus", "list")
        help_text = run("fo-corpus", "sec10k", "--help")
        for option in ("--user-agent", "--cik", "--filings-per-company"):
            if option not in help_text:
                raise RuntimeError(f"SEC acquisition option missing: {option}")
        run("fo-corpus", "gutenberg", "--help")
        run("fo-corpus", "verify", work / "missing-corpus", success=False)
        print("PASS: installed CLIs, index round-trip, edited-passage attribution, "
              "corrupt-index rejection, and corpus command surface")


if __name__ == "__main__":
    main()
