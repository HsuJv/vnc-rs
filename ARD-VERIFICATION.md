# ARD Authentication: Real-Machine Verification Guide

Goal: check whether the Apple ARD authentication (RFB security type 30) added in
this repository can connect to a real macOS Screen Sharing service and get past
the login window that plain VNC authentication hits on macOS 10.7 and later.

Protocol logic is already covered by the fake-server tests (`cargo test`). This
document only covers interoperability against a real Mac.

## 1. Prepare the test machine

Requirements: Linux or Windows, on the same LAN as the MacBook, able to reach
port 5900 on the Mac.

```bash
# Install Rust (skip if already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Get the code (the ARD changes are on the main branch)
git clone https://github.com/michael-eddy/vnc-rs.git
cd vnc-rs

# Sanity check: the fake-server unit and integration tests must all pass
cargo test
```

If `cargo test` is green, the code and toolchain are fine and you can move on to
the real-machine steps. On Windows, install Rust through rustup-init and run the
same commands from a Git Bash shell.

## 2. Configure the MacBook (host being controlled)

1. Turn on Screen Sharing:
   - macOS 13 and later: System Settings -> General -> Sharing -> enable Screen Sharing
   - older macOS: System Preferences -> Sharing -> check Screen Sharing
2. Do **not** check "VNC viewers may control screen with password".
   With it checked the server also offers type 2 (plain VNC authentication);
   without it the server offers only type 30 (ARD), which is the scenario under
   test.
3. Confirm the account you will use is allowed in: the ⓘ next to Screen Sharing
   -> "Allow access for" -> All users, or a list that includes that account
   (administrators are allowed by default).
4. Note the Mac's LAN address (the sharing pane shows `vnc://192.168.x.x`, or
   run `ifconfig | grep "inet "`).
5. Firewall: System Settings -> Network -> Firewall -> allow Screen Sharing
   (or switch the firewall off for the duration of the verification).
6. Keep the Mac awake: system sleep drops the connection, display sleep does not.
   On AC power, `caffeinate -dis` works.

## 3. Verification steps

Run every command below from the repository root on the test machine. The first
run compiles the dependencies and takes a few minutes.

### 3.1 Probe the security types the server offers

```bash
cargo run --manifest-path example/Cargo.toml --bin ard_probe -- <mac-ip>
```

Expected output. A real macOS server lists several Apple types at once; seeing
`003.889`, `apple server: true` and a 30 in the list is normal, and this
repository does not use the other types.

```text
server version: "RFB 003.889\n"
apple server:   true
offered security types: 30 (Apple ARD), 33 (Apple (legacy SRP/DH)), 36 (Apple SRP), 31 (unknown), 32 (unknown), 35 (Apple (legacy SRP/DH))
=> type 30 (Apple ARD) is available; retry with --connect <username> <password>
```

If `2 (VNC auth)` also appears in the list, the "VNC viewers may control screen
with password" option is checked, which is the fallback case in step 3.5.

### 3.2 Correct credentials: full ARD connection (the core check)

```bash
cargo run --manifest-path example/Cargo.toml --bin ard_probe -- <mac-ip> --connect <username> <password>
```

Use the short account name printed by `id -un` on the Mac, and that account's
login password. Quote the password so the shell does not interpret `$` or `!`.

Expected output:

```text
ARD authentication succeeded in ..., desktop name: "..."
resolution: ...
first frame: rect ..., ... bytes
success: ARD login bypassed the login window and got a frame in ...
```

Getting a first frame means ARD login succeeded and the pixel channel works,
which is the core objective.

### 3.3 Wrong password: the failure path

```bash
cargo run --manifest-path example/Cargo.toml --bin ard_probe -- <mac-ip> --connect <username> wrongpass
```

Expected: `ARD authentication failed: <reason from the server>` and a non-zero
exit status. It must not hang, and it must not report success.

### 3.4 Locked screen / login window

Lock the Mac (Control-Command-Q), or log out down to the login window, then
repeat 3.2.

Expected: still successful. Nobody needs to be logged into the desktop first,
and that is exactly what ARD buys over plain VNC authentication.

### 3.5 (Optional) fallback path

On the Mac, check "VNC viewers may control screen with password" and set a VNC
password (the client never uses this password; it only makes the server advertise
type 2), then:

- repeat 3.1: both `2 (VNC auth)` and `30 (Apple ARD)` should be listed;
- repeat 3.2: it should still prefer 30 and succeed, with the same output as 3.2.

When you are done, clear that checkbox again to leave the Mac as you found it.

## 4. Symptom table

| Symptom | Likely cause | What to do |
| --- | --- | --- |
| 3.1 connection refused / times out | Screen Sharing off, firewall, wrong address, different subnet | Re-check section 2; confirm reachability with `ping <mac-ip>` |
| Version string is not `003.889` | The peer is not the built-in macOS service (possibly a third-party VNC server) | Make sure you are talking to port 5900 on the Mac itself |
| Only `2 (VNC auth)` is offered | "VNC viewers may control screen with password" is checked | Clear the checkbox and repeat 3.1 |
| Message that the server does not offer ARD | Same as above, or the server genuinely does not offer 30 | Record the full 3.1 output and report it |
| 3.2 fails with `ArdAuthFailed` although the password is right | The account is not allowed to use Screen Sharing, or an interoperability problem | Rerun with `-vv` and record the output |
| Authentication succeeds but no first frame within 10 seconds | Post-authentication initialization or encoding negotiation | Rerun with `-vv` and record the output; also check whether the Mac screen is showing a prompt asking to approve the sharing request |

## 5. If it fails, report these

1. The macOS version (About This Mac).
2. The full output of 3.1.
3. The full output of the failing step with `-vv`, for example:

   ```bash
   cargo run --manifest-path example/Cargo.toml --bin ard_probe -- <mac-ip> --connect <username> <password> -vv
   ```

Note: `-vv` trace logs can contain sensitive data. Redact them before reporting,
and never include the password.

## 6. Results

All identifying details are redacted: no real address, account name, desktop
name or password is recorded here, only the criteria that make the result
reproducible.

- Host: the built-in macOS Screen Sharing service (`<mac-ip>`, non-empty desktop
  name returned by the server, resolution 2880x1800)
- Test machine: Windows 10, same subnet as the host
- Date: 2026-11-11

| Step | Result | Notes |
| --- | --- | --- |
| 3.1 probe | pass | `RFB 003.889`, `apple server: true`; with the VNC password option cleared the server offers only 30/33/36/31/32/35, no type 2 |
| 3.2 correct credentials | pass | authentication 749 ms, first frame at 1.63 s (Raw 64x64 = 16384 bytes), exit 0 |
| 3.3 wrong credentials | pass | returns `ARD authentication failed: Authentication or authorization failure` immediately, exit 1, no hang, no false success |
| 3.4 locked screen | pass | left at the login window with nobody logged in; authenticated in 749 ms and delivered a frame, with no local approval required |
| 3.5 fallback (optional) | pass | with the VNC password option set, type 2 and 30 are both offered, the log still reads `Apple ARD auth done`, authentication 282 ms, first frame at 1.09 s |

Conclusions:

1. The server accepts the DH public key and the AES-encrypted credentials this
   client sends, and answers with a readable application-level reason on failure
   instead of dropping the connection, so the ARD handshake format interoperates
   with a real Mac.
2. On the Windows test machine,
   `cargo build --manifest-path example/Cargo.toml --bin ard_probe` succeeded on
   the first try (including the `minifb` dependency), so the Linux requirement
   in section 1 is not essential.
3. Known harmless noise: after `close()` the decoder task logs one
   `ERROR Error occurs during the decoding General("Channel closed")`. It does
   not affect the exit status or the verdict.
