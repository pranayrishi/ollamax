# Security policy

## Supported versions

Ollama-Forge is pre-alpha (v0.1.x). Only `main` is supported. There are no
backports.

## Reporting a vulnerability

**Please do not file public GitHub issues for security bugs.**

Email: security@ollama-forge.invalid (replace `.invalid` with the real domain
once one exists — until then, open a private security advisory on GitHub:
https://github.com/ollama-forge/ollama-forge/security/advisories/new).

Include:

- A description of the issue and its impact.
- Steps to reproduce, ideally with a minimal `forge.toml` and command line.
- Affected commit hash (`git rev-parse HEAD`).
- Your environment: OS, architecture, Ollama version, model in use.

We aim to acknowledge reports within 5 business days.

## Threat model

Ollama-Forge is designed to keep your code on your machine. The threats we
care about, in priority order:

1. **Accidental exfiltration of secrets** to a model. The bundled
   `SecurityGuard` scans content for credential patterns before sending it to
   Ollama. Reports of bypasses (regex evasions, encoding tricks, novel
   credential formats) are in scope.
2. **Untrusted skill recipes.** Skill JSON files can carry arbitrary prompts.
   A malicious recipe that tricks the model into running dangerous shell
   commands is in scope. The current mitigation is the `Dangerous Shell
   Commands` rule in `src/security/mod.rs`; bypasses count.
3. **Outbound network calls.** The binary should make exactly one class of
   network call: HTTP to the configured `ollama_url`. Any other outbound
   request is a bug.
4. **RCE in the binary itself** via crafted Ollama responses, malformed
   `forge.toml`, or skill JSON.

## Out of scope

- Anything requiring physical access to the machine.
- Vulnerabilities in Ollama itself — please report those upstream at
  https://github.com/ollama/ollama/security/advisories.
- Vulnerabilities in models you choose to run.
