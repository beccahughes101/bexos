# RFC-0073: Platform SSO Architecture, Sandboxed WASM Identity Plugins, and Hardware-Bound Passwordless Storage Decryption

* **Author:** BexOS Identity, Security & Cryptography Working Group
* **Status:** Proposed
* **Target Subsystems:** `usersd`, `sysui`, `trusty` (TEE), `bexfs`, `sdk/fidl/bexos.identity`
* **Applicability:** Enterprise SSO, FIDO2/WebAuthn PRF Passwordless Logins, OIDC/SAML Federation, Per-User Storage Encryption (UKEK)

---

## 1. Summary

This RFC specifies the unified authentication and identity management framework for BexOS. It introduces:

1. **Platform Single Sign-On (PSSO) Engine:** A pluggable authentication architecture within `usersd` running sandboxed, in-process WebAssembly (WASM) identity plugins for protocols including local passwords, LDAP/Kerberos, FIDO2/WebAuthn, and OpenID Connect (OIDC).
2. **Multi-Step Authentication Protocol:** An asynchronous, interactive challenge-response pipeline supporting interactive terminal flows, multi-factor authentication (MFA), and sandboxed web-based federated sign-in brokered via `sysui`.
3. **Synthetic Gatekeeper Secret Wrapping:** A cryptographic decoupling layer allowing `IS_PASSWORDLESS` and federated OIDC users to transparently unlock hardware-backed disk encryption keys (User Key Encryption Keys, or UKEK) managed by Trusty Gatekeeper without prompting for legacy passwords.
4. **FIDO2 PRF Key Decryption:** Passwordless hardware-authenticator login using the WebAuthn Pseudo-Random Function (PRF) extension to locally unseal synthetic Gatekeeper credentials.
5. **Trusty Token Exchange & Key Wrap App:** A secure world application running in the Trusty TEE that manages device-bound enrollment keys, validates signed JWT identity assertions against stored trust roots, and releases wrapped disk unlock secrets.

---

## 2. Motivation

Traditional Unix and desktop authentication frameworks (such as Linux PAM) exhibit several architectural flaws in microkernel and capability-based operating systems:

* **In-Process Unsandboxed C Plugins:** Standard PAM modules execute arbitrary, unconfined C code with root privileges inside the login daemon. Vulnerabilities in LDAP, Kerberos, or JSON parsers lead directly to ring-3 privilege escalation.
* **Identity vs. Storage Decryption Mismatch:** Enterprise identity providers (Google Workspace, Microsoft Entra ID, Okta) issue ephemeral, short-lived tokens (e.g., OIDC JSON Web Tokens). In contrast, File-Based Encryption (FBE) requires a static, high-entropy cryptographic symmetric key (the UKEK) to derive per-inode file encryption keys. Tying disk unlock to transient passwords forces enterprise users to manage duplicate credentials (a "login password" vs. an "IdP password") or causes login failures when offline.
* **Lack of First-Class Passwordless Primitives:** Legacy systems treat hardware tokens (YubiKeys) as secondary 2FA layers rather than primary, standalone authentication and disk decryption credentials.

BexOS resolves these problems by executing identity modules in isolated WebAssembly runtimes, utilizing FIDO2 PRF extensions to unlock synthetic secrets, and brokering web authentication through an out-of-process sandboxed UI.

---

## 3. High-Level Architecture & Plane Separation

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ PRESENTATION LAYER: `sysui` (Login Screen / Display Server)                 │
│ • Displays user tiles, credential prompts, and IdP selection                │
│ • Spawns sandboxed, isolated WebViews for federated OIDC flows              │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL: `bexos.identity.LoginSession`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ AUTHENTICATION LAYER: `usersd` (D1 User & Identity Daemon)                   │
│                                                                             │
│  [ Users Registry ]                                                         │
│  • Manages local/remote user records, PSSO mappings, and credential arrays │
│                                                                             │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │ In-Process Sandboxed WASM Plugins (Wasmtime)                          │  │
│  │ • `psso_local_password.wasm`                                          │  │
│  │ • `psso_fido2_prf.wasm`                                               │  │
│  │ • `psso_oidc_google.wasm` / `psso_oidc_microsoft.wasm`                │  │
│  └───────────────────────────────────┬───────────────────────────────────┘  │
└──────────────────────────────────────┼──────────────────────────────────────┘
                                       │ TIPC (Trusty IPC)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ HARDWARE TRUST ROOT: Trusty TEE (Secure World / EL3 / Secure Partition)     │
│                                                                             │
│  [ Gatekeeper Applet ]                                                      │
│  • Enforces rate-limiting, exponential backoff, and RPMB replay protection  │
│  • Derives UKEK (User Key Encryption Key) for `bexfs` home directory        │
│                                                                             │
│  [ Token Exchange Applet ]                                                  │
│  • Stores hardware-bound device private keys (P-256 / Ed25519)              │
│  • Verifies IdP JWT signatures against pre-provisioned trusted public keys  │
│  • Unwraps device-bound synthetic Gatekeeper secrets                         │
└─────────────────────────────────────────────────────────────────────────────┘

```

---

## 4. User Record & Credential Data Model

User definitions are stored in `/data/system/users.db` by `usersd`. Secrets are never stored in plaintext; they are stored as Gatekeeper handles or encrypted blobs.

```rust
pub struct UserRecord {
    pub uid: u32,
    pub username: String,
    pub account_type: AccountType, // Local | Remote
    pub is_passwordless: bool,
    pub primary_psso_id: String,   // e.g. "bexos.psso.oidc.google"
    pub fallback_psso_id: Option<String>,
    pub credentials: Vec<CredentialBinding>,
}

pub enum AccountType {
    Local,
    Remote { idp_domain: String },
}

pub enum CredentialBinding {
    /// Password verified and rate-limited inside Trusty Gatekeeper
    Password {
        gatekeeper_handle: u64,
        salt: [u8; 32],
    },
    /// FIDO2 / WebAuthn hardware token using HMAC-Secret / PRF extension
    SecurityKey {
        credential_id: Vec<u8>,
        relying_party_id: String,
        prf_salt: [u8; 32],
        /// High-entropy synthetic Gatekeeper password encrypted via AES-256-GCM
        /// Key derived from: HKDF-SHA256(PRF_Output, prf_salt)
        encrypted_synthetic_password: Vec<u8>,
        nonce: [u8; 12],
    },
    /// Federated OIDC Identity
    OidcBinding {
        issuer: String,
        subject: String,
        /// TEE Key Slot index holding the device-wrapped synthetic secret
        tee_key_slot: u32,
        cached_profile_info: Vec<u8>,
    },
}

```

---

## 5. Synthetic Password Decoupling & Passwordless Unlocking

Trusty Gatekeeper requires a high-entropy secret to release the User Key Encryption Key (UKEK) that decrypts the user's home directory in `bexfs`.

For users where `is_passwordless == true` (e.g., WebAuthn or OIDC), the platform synthesizes an internal 256-bit random secret. The user never sees, types, or manages this secret.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ SYNTHETIC PASSWORD DERIVATION PIPELINE                                      │
│                                                                             │
│ 1. Enrollment:                                                              │
│    `usersd` requests 32 cryptographically secure random bytes:              │
│    `synthetic_password = zx_cprng_draw(32)`                                 │
│    This secret is enrolled into Trusty Gatekeeper as the primary secret     │
│    governing the user's UKEK.                                               │
│                                                                             │
│ 2. Sealing via FIDO2 PRF:                                                   │
│    `usersd` passes a 32-byte salt to the physical YubiKey via CTAP 2.1.     │
│    `prf_output = Authenticator.EvaluatePRF(salt)`                           │
│    `derived_aes_key = HKDF(prf_output, salt)`                               │
│    `encrypted_synthetic_password = AES_GCM_Encrypt(derived_aes_key,         │
│                                                    synthetic_password)`     │
│                                                                             │
│ 3. Unsealing at Login:                                                      │
│    User taps hardware authenticator.                                        │
│    `prf_output = Authenticator.EvaluatePRF(salt)`                           │
│    `derived_aes_key = HKDF(prf_output, salt)`                               │
│    `synthetic_password = AES_GCM_Decrypt(derived_aes_key,                   │
│                                          encrypted_synthetic_password)`     │
│    `synthetic_password` is submitted to Trusty Gatekeeper to release UKEK.  │
└─────────────────────────────────────────────────────────────────────────────┘

```

---

## 6. Detailed Authentication Flows

### 6.1 Flow A: Local or Cached Password

1. User enters username and password in `sysui`.
2. `usersd` routes the credential to `psso_local_password.wasm`.
3. The WASM plugin normalizes the input and forwards the verification payload to `usersd`.
4. `usersd` issues a TIPC command to Trusty Gatekeeper. Gatekeeper checks password validity, updates monotonic failure counters in RPMB, and—if valid—returns the UKEK.

### 6.2 Flow B: Local FIDO2 / WebAuthn PRF (Passwordless)

1. User selects their profile on the login screen.
2. `usersd` identifies `is_passwordless == true` and selects the `SecurityKey` binding.
3. `sysui` displays an on-screen prompt: *"Touch your security key"*.
4. `usersd` communicates with the hardware token over USB/NFC via `bexos.hardware.usb.hid`.
5. The authenticator executes the PRF evaluation and yields the 32-byte symmetric output.
6. `psso_fido2_prf.wasm` unseals `encrypted_synthetic_password` in memory, passes it to Trusty Gatekeeper, and releases the UKEK.

### 6.3 Flow C: Federated OIDC Authentication & Trusty Token Exchange

OIDC ID tokens are ephemeral and cannot directly serve as persistent disk encryption keys. BexOS addresses this using the **Device-Bound Wrapped Secret Pattern**:

```
`sysui` (Login UI)          `usersd` (WASM OIDC)     IdP (Google/Okta)       Trusty TEE (Token Exchanger)
      │                             │                        │                            │
      │ 1. Select "Google Login"    │                        │                            │
      ├────────────────────────────►│                        │                            │
      │                             │ 2. Return Step::Web    │                            │
      │                             │    { auth_url }        │                            │
      │◄────────────────────────────┤                        │                            │
      │                                                      │                            │
      │ 3. Spawn sandboxed WebView runner                    │                            │
      │    Load auth_url                                     │                            │
      ├─────────────────────────────────────────────────────►│                            │
      │ 4. User completes IdP sign-in, MFA, Passkeys         │                            │
      │◄─────────────────────────────────────────────────────┤                            │
      │                                                      │                            │
      │ 5. Intercept redirect URL                            │                            │
      │    Extract Auth Code / JWT                           │                            │
      ├────────────────────────────►│                        │                            │
      │                             │ 6. Exchange code for   │                            │
      │                             │    signed ID Token     │                            │
      │                             ├───────────────────────►│                            │
      │                             │◄───────────────────────┤                            │
      │                             │                                                     │
      │                             │ 7. Submit JWT to TEE                                │
      │                             │    TIPC: `ExchangeToken(jwt, slot_id)`             │
      │                             ├────────────────────────────────────────────────────►│
      │                             │                                                     │ 8. Validate JWT sig
      │                             │                                                     │    against cached IdP cert
      │                             │                                                     │ 9. Verify `sub` claim
      │                             │                                                     │    matches enrolled user
      │                             │                                                     │ 10. Unwrap synthetic
      │                             │                                                     │     Gatekeeper password
      │                             │                                                     │     via Device Master Key
      │                             │ 11. Return UKEK                                     │
      │                             │◄────────────────────────────────────────────────────┤

```

* **Offline Login Resilience:** If the network is unreachable, `sysui` detects link absence and dims the OIDC option. The user is prompted to authenticate via their enrolled **Offline Fallback Credential** (FIDO2 PRF security key or secondary local PIN).

---

## 7. The Multi-Step Authentication Protocol (FIDL)

Authentication steps are negotiated dynamically between `usersd` and `sysui` over `idl/bexos/identity/login.fidl`:

```fidl
library bexos.identity;

using bexos.kernel;

type AuthStepType = flexible union {
    1: password struct {
        prompt_text string:128;
    };
    2: security_key struct {
        relying_party_id string:256;
        credential_ids vector<vector<uint8>:64>:8;
    };
    3: web_redirect struct {
        initial_url string:2048;
        redirect_intercept_prefix string:256;
    };
    4: biometric struct {
        prompt_text string:128;
        allow_pin_fallback bool;
    };
};

type AuthStepResult = flexible union {
    1: password_response string:256;
    2: security_key_response struct {
        credential_id vector<uint8>:64;
        prf_result vector<uint8>:32;
    };
    3: web_response struct {
        final_redirect_url string:4096;
    };
    4: biometric_response struct {
        auth_token_handle zx.Handle:CHANNEL;
    };
};

@discoverable
protocol LoginSession {
    /// Initiates an authentication session for a given user or discovery prompt
    BeginSession(struct {
        username_hint box<string:128>;
    }) -> (resource struct {
        first_step AuthStepType;
    }) error bexos.kernel.Status;

    /// Submits response for current step; returns next step or completes session
    AdvanceSession(resource struct {
        result AuthStepResult;
    }) -> (resource struct {
        next_step box<AuthStepType>; // Null if authentication complete
    }) error bexos.kernel.Status;

    /// Finalizes authentication and mounts the user's encrypted home directory
    CompleteSession() -> (resource struct {
        user_token zx.Handle:CHANNEL;
    }) error bexos.kernel.Status;
};

```

---

## 8. Web UI Sandboxing in `sysui`

`usersd` remains completely headless and runs zero HTML, CSS, or JavaScript parsers.

When a PSSO plugin returns an authentication step of type `AuthStepType::WebRedirect`:

1. `usersd` dispatches the step to `sysui`.
2. `sysui` asks `appd` to launch a dedicated, ephemeral `webview_runner` component inside an unprivileged sub-job.
3. The WebView process is granted only two capabilities:
* Direct packet socket egress over `bexos.net.SocketProvider` to reach the IdP endpoint.
* A shared `VMO` frame token connecting to `scened` to display the authentication frame.


4. The WebView process has no access to local filesystems, host IPC endpoints, or peripheral devices.
5. When the user completes authentication and the browser navigates to the pre-configured `redirect_intercept_prefix` (e.g., `[https://bexos.local/sso/callback#](https://bexos.local/sso/callback#)...`), `sysui` captures the URL payload, immediately destroys the WebView job sandbox, and returns the tokens to `usersd`.

---

## 9. Biometrics as an Escalation Layer

Biometric authentication (fingerprint, 3D face recognition via `fingerprintd`) is explicitly designed as an **ephemeral unlock layer**, not a root enrollment credential:

1. **Cold Boot Requirement:** On cold system boot or full user logoff, biometrics are disabled. The primary PSSO method (Password, FIDO2 Security Key, or OIDC) must be performed to unseal the synthetic Gatekeeper secret and release the UKEK from Trusty.
2. **Session Key Caching:** Upon successful primary login, Trusty retains the UKEK in secure on-chip SRAM and issues a temporary `AuthToken` signed with an ephemeral HMAC key.
3. **Screen Unlock:** When unlocking a suspended or locked session:
* `sysui` prompts for biometrics.
* `fingerprintd` captures matching sensor vectors and submits them directly to the Trusty Biometric applet over an isolated hardware SPI/I2C capability.
* Trusty verifies the biometric template against secure RPMB storage, validates the `AuthToken`, and signals `usersd` to wake the session without re-evaluating the primary PSSO pipeline.



---

## 10. Security Analysis & Threat Matrix

| Threat Scenario | System Defense / Mitigation |
| --- | --- |
| **Malicious or Compromised PSSO Plugin** | Plugins run inside Wasmtime memory sandboxes with fuel meters and epoch-based preemption. A bug or exploit in a plugin cannot corrupt `usersd` heap memory or access unassigned capabilities. |
| **IdP Token Interception / Replay Attack** | Trusty Token Exchange verifies token nonces and expiration windows (`exp`). The unwrapping of the synthetic password is bound to the device's hardware-fused public key; intercepted tokens replayed on a different physical device cannot unwrap the local UKEK. |
| **Physical Security Key Sniffing** | Communication with hardware authenticators uses standard FIDO2 CTAP 2.1 channel encryption. PRF outputs are generated on-device within the secure element of the physical security key and are wiped from memory immediately after deriving the Gatekeeper secret. |
| **Brute-Force Lockout Attacks** | Password inputs are serialized into Trusty Gatekeeper. Monotonic counters in RPMB hardware flash enforce progressive throttling (e.g., 5 attempts, then 30s timeout, up to 24-hour hardware lockouts). |
| **Phishing / Rogue Redirects in Web SSO** | The `webview_runner` matches navigation targets strictly against a domain allowlist defined in the signed PSSO manifest. Rogue redirects outside the identity domain halt execution immediately. |

---

## 11. Implementation Roadmap

### Phase 1: Core Protocol & WASM Plugin Engine in `usersd`

* Implement `bexos.identity.LoginSession` FIDL protocol.
* Integrate Wasmtime within `services/usersd` to host modular PSSO plugins.
* Build native `psso_local_password` and FIDO2 CTAP 2.1 (`psso_fido2_prf`) plugins in Rust.

### Phase 2: Trusty Secure World Components

* Implement the Synthetic Secret Wrapping interface in Trusty Gatekeeper.
* Build the `token_exchange` Trusty Secure Applet to store device keypairs and unwrap secrets against validated JWT payloads.
* Integrate RPMB monotonic counter storage for replay-protected enrollment mappings.

### Phase 3: UI Brokering & Out-of-Tree Packaging

* Implement the ephemeral `webview_runner` bridge inside `sysui` to handle `AuthStepType::WebRedirect`.
* Build sample enterprise PSSO plugins for Google Workspace and Microsoft Entra ID.
* Verify end-to-end passwordless workstation enrollment, cold boot, and home directory decryption via YubiKey PRF on bare metal.