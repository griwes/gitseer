#!/usr/bin/env python3
"""Check locked Cargo packages for an Apache-compatible license choice."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from dataclasses import dataclass

APPROVED_LICENSES = {
    "Apache-2.0",
    "CC0-1.0",
    "ISC",
    "MIT",
    "Unicode-3.0",
    "Unlicense",
    "Zlib",
}
APPROVED_EXCEPTIONS = {"LLVM-exception"}
TOKEN = re.compile(r"\(|\)|AND|OR|WITH|[A-Za-z0-9][A-Za-z0-9.+-]*")


def tokenize(expression: str) -> list[str]:
    normalized = expression.replace("/", " OR ")
    tokens: list[str] = []
    position = 0

    for match in TOKEN.finditer(normalized):
        if normalized[position : match.start()].strip():
            raise ValueError(f"unsupported SPDX syntax near {normalized[position:]!r}")
        tokens.append(match.group(0))
        position = match.end()

    if normalized[position:].strip():
        raise ValueError(f"unsupported SPDX syntax near {normalized[position:]!r}")

    return tokens


@dataclass
class Parser:
    tokens: list[str]
    position: int = 0

    def parse(self) -> bool:
        result = self.parse_or()
        if self.position != len(self.tokens):
            raise ValueError(f"unexpected SPDX token {self.tokens[self.position]!r}")
        return result

    def parse_or(self) -> bool:
        result = self.parse_and()
        while self.accept("OR"):
            alternative = self.parse_and()
            result = result or alternative
        return result

    def parse_and(self) -> bool:
        result = self.parse_with()
        while self.accept("AND"):
            requirement = self.parse_with()
            result = result and requirement
        return result

    def parse_with(self) -> bool:
        result = self.parse_primary()
        if self.accept("WITH"):
            exception = self.take()
            result = result and exception in APPROVED_EXCEPTIONS
        return result

    def parse_primary(self) -> bool:
        if self.accept("("):
            result = self.parse_or()
            self.expect(")")
            return result

        license_id = self.take()
        if license_id in {"AND", "OR", "WITH", "(", ")"}:
            raise ValueError(f"expected license id, found {license_id!r}")
        return license_id in APPROVED_LICENSES

    def accept(self, token: str) -> bool:
        if self.position < len(self.tokens) and self.tokens[self.position] == token:
            self.position += 1
            return True
        return False

    def expect(self, token: str) -> None:
        if not self.accept(token):
            found = self.tokens[self.position] if self.position < len(self.tokens) else "<end>"
            raise ValueError(f"expected {token!r}, found {found!r}")

    def take(self) -> str:
        if self.position >= len(self.tokens):
            raise ValueError("unexpected end of SPDX expression")
        token = self.tokens[self.position]
        self.position += 1
        return token


def compatible(expression: str) -> bool:
    tokens = tokenize(expression)
    if not tokens:
        raise ValueError("empty SPDX expression")
    return Parser(tokens).parse()


def cargo_packages() -> list[dict[str, object]]:
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    metadata = json.loads(result.stdout)
    return metadata["packages"]


def main() -> int:
    failures: list[str] = []
    packages = cargo_packages()

    for package in packages:
        name = str(package["name"])
        version = str(package["version"])
        expression = package.get("license")

        if not isinstance(expression, str) or not expression.strip():
            failures.append(f"{name} {version}: missing SPDX license expression")
            continue

        try:
            approved = compatible(expression)
        except ValueError as error:
            failures.append(f"{name} {version}: {expression!r}: {error}")
            continue

        if not approved:
            failures.append(f"{name} {version}: no approved path in {expression!r}")

    if failures:
        print("dependency license audit failed:", file=sys.stderr)
        for failure in sorted(failures):
            print(f"  - {failure}", file=sys.stderr)
        return 1

    print(f"dependency license audit passed for {len(packages)} locked packages")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

