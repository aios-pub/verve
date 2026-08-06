# Security & Credentials

This document explains how Verve stores secrets (SSH passwords, private-key
passphrases, etc.), why it does **not** prompt you with OS keychain
authorization dialogs, and how the design compares to other SSH clients.

---

## Overview

A guiding principle of Verve: **secrets are never written in plaintext to host
configuration files.** The host list (`~/.verve/ssh_hosts.json`) stores only
non-sensitive connection metadata (address, port, username, auth type). Every
password and key passphrase lives in a separate **encrypted vault**, sealed
with a per-device key.

The design goal is the "Termius-like" experience: **no master password to
remember, and no per-access OS keychain authorization dialog** — while keeping
secrets encrypted at rest and isolated per OS user.

---

## How credentials are stored

| File | Purpose | Permissions (Unix) |
|---|---|---|
| `~/.verve/ssh_hosts.json` | Host connection metadata (no secrets) | default |
| `~/.verve/device.key` | 32-byte random device key | `0600` |
| `~/.verve/ssh_vault.bin` | AES-256-GCM encrypted secrets | `0600` |

Flow:

1. On first run, Verve generates a random 256-bit **device key** and writes it
   to `~/.verve/device.key` (mode `0600` on Unix — readable only by your user).
2. Every SSH password / key passphrase is stored as an entry inside the
   **vault** (`~/.verve/ssh_vault.bin`), encrypted with the device key using
   **AES-256-GCM** (authenticated encryption). The vault file format is
   `[12-byte nonce][ciphertext + GCM tag]`.
3. At connect time, the vault is decrypted once in memory (and cached for the
   rest of the process), the needed secret is read out, and the device key is
   zeroized.

Because the device key never leaves the local machine and the vault is
authenticated-encrypted, an attacker who only obtains `ssh_hosts.json` gains
nothing; an attacker who obtains `ssh_vault.bin` without `device.key` cannot
decrypt it.

---

## Why no master password / no keychain prompt

Earlier versions of Verve (and many other SSH clients) stored secrets in the
OS keychain. On macOS this routinely causes two pain points:

- **Authorization dialog on every access.** macOS Keychain enforces per-item
  access control lists (ACLs) keyed on the calling app's code signature. For
  apps signed ad-hoc or rebuilt frequently (typical for open-source builds),
  the signature hash (`cdhash`) changes on every build, so macOS no longer
  recognizes the app as the same trusted caller — the "Always Allow" choice is
  forgotten and a dialog pops up again on the next access.
- **Master-password friction.** Some apps avoid the keychain by encrypting a
  vault with a master password the user must type at startup — secure, but
  tedious.

Verve's device-key design sidesteps both:

- **No keychain access** → no ACL → no authorization dialog. Ever.
- **No master password** → the device key is auto-generated and stored as a
  protected file; the user never types it.

This mirrors the approach of commercial clients like Termius, which likewise
maintain their own encrypted store and never trigger the OS keychain prompt.

---

## Per-module credential sources

| Module | Where its secrets live | When the vault is read |
|---|---|---|
| **SSH** (password / key passphrase) | Device-key encrypted vault | On every new connection (cached after first decrypt) |
| **Docker** (SSH-tunnel mode) | Reuses the SSH host's vault secret | Only when first opening the SSH tunnel (tunnels are cached per host) |
| **Kubernetes** (SSH-tunnel mode) | Reuses the SSH host's vault secret | Only when first opening the API tunnel (cached per cluster) |
| Kubernetes API auth (token / client-cert / basic) | Plaintext in `~/.verve/kube_clusters.json` + temporary kubeconfig | Not in the vault |
| Docker local / TLS | Environment variables (`DOCKER_HOST`, `DOCKER_CERT_PATH`, …) | Not in the vault |
| Git token | Memory only (never persisted) | Not stored |

> **Note:** Kubernetes API tokens and Git tokens are **not** yet integrated
> with the vault — they are stored as plaintext or kept only in memory. Moving
> them into the vault is tracked as future work.

---

## Security considerations

- **File permissions.** `device.key` and `ssh_vault.bin` are written with mode
  `0600` on Unix (owner read/write only). On Windows the per-user profile ACL
  provides equivalent isolation. The `~/.verve/` directory is created with
  `0700`.
- **Memory hygiene.** The device key is [`zeroize`](https://docs.rs/zeroize)d
  after use. The decrypted secret map is cached in process memory for the
  lifetime of the app (so the vault is decrypted at most once per run); this
  cache is not persisted to disk.
- **Encryption.** AES-256-GCM provides both confidentiality and integrity
  (authenticated encryption). A fresh random nonce is generated on every
  write, giving forward secrecy for the on-disk ciphertext.
- **Trust boundary.** The design protects against other non-privileged users
  on the same machine and against loss of the host config file. It does **not**
  protect against an attacker with root/admin privileges or physical access to
  an unlocked session — that class of attacker can read the device key from
  disk or dump process memory. (Hardware-backed protection via the Secure
  Enclave is a possible future enhancement.)

---

## Migrating from earlier keychain-based builds

If you are upgrading from a build that stored SSH secrets in the OS keychain,
those secrets will not be readable by the new vault. Re-enter the password /
passphrase once when editing each affected host — it will then be stored in the
encrypted vault going forward. You can clean up the old keychain entries
manually via Keychain Access (macOS), Credential Manager (Windows), or
`secret-tool` (Linux) by searching for the service name `verve.ssh`.
