# RFC 0002: Native AI architecture

- Created: 2026-08-26T17:31:23-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

BexOS integrates AI through capability-gated FIDL tools, transient WASM interfaces, compositor semantics, CEL policies, and encrypted per-user memory.

## Design overview

BexOS's capability security, FIDL IPC, and WASM runtimes allow AI to interact directly with the OS's semantic interfaces. This design makes AI an architectural layer instead of relying on an overlay chatbot or screen scraping.

## Semantic IPC: Type-Safe Tool Use via FIDL

Current AI OS assistants (like Apple Intelligence or Windows Copilot/Recall) rely on fragile accessibility APIs, OCR, or simulated mouse clicks to interact with apps.

In BexOS, every app already publishes FIDL interfaces and Extension Points to `appd`. A **Semantic Manifest Annotation** describes those interfaces for AI use:

* **How it works:** Apps annotate their existing FIDL methods with natural-language schemas (docstrings and parameter semantics).
* **The OS AI Broker:** The system AI service inspects the system's live capability graph and compiles FIDL schemas into dynamic tool-calling definitions.
* **Direct RPC Execution:** For a request such as *"Book the cheapest flight from my open tab and log the expense in Monzo,"* the AI does not simulate clicks. It executes verified, type-safe FIDL calls directly across channel handles with zero latency and zero UI fragility.

```fidl
library bexos.finance.expenses;

protocol ExpenseTracker {
    /// @ai.intent("Logs an expense with an amount, currency, merchant, and category")
    LogExpense(struct {
        amount uint64,
        currency string:3,
        merchant string:64,
        category string:32
    }) -> (struct { status Status, transaction_id uint64 });
};

```

## JIT Transient UI (On-The-Fly WASM App Synthesis)

Traditional operating systems force the user to juggle multiple distinct apps to accomplish a compound task.

With an embedded WASM runner and a composable UI framework, BexOS can synthesize **ephemeral, throwaway UI widgets** on demand:

* **Example request:** *"Compare these 3 rental listings I'm viewing, calculate commute times to my office, and let me vote on them with Sarah."*
* **The OS Response:** The AI generates a lightweight, sandboxed WASM UI component rendered directly into a floating surface by the BexOS Compositor.
* **Ephemeral Lifecycle:** The widget is temporary, backed by live FIDL data feeds from the browser, maps service, and messaging channels. Closing its window after a decision discards the container; it is not installed as a permanent app.

## The Compositor-Level "Visual & Semantic Scene Graph"

Existing OSs process multimodal visual context by taking high-res screenshots and sending uncompressed bitmaps to an NPU or cloud server—killing battery and leaking private pixels.

Because the BexOS Compositor controls window rendering via WebGPU/shared VMOs:

* **Vector & Node Awareness:** The compositor already maintains the structured DOM/widget tree (text nodes, bounding boxes, element types) for all active surfaces.
* **Zero-Copy NPU Feed:** Instead of OCR, the compositor streams a lightweight, anonymized vector representation of the screen directly to the local NPU service via a zero-copy memory ring.
* **Zero-Latency Awareness:** The AI instantly knows the screen contents with near-zero CPU and memory overhead, without ever capturing raw framebuffers.

## Deterministic CEL Guardrails (The AI Safety Firewall)

The biggest hazard of autonomous OS agents is the "confused deputy" attack—a malicious email or website prompt-injecting the AI to delete files or exfiltrate tokens.

BexOS natively solves this at the microkernel and `appd` layer:

* **The AI Agent is an Unprivileged Service:** The AI engine holds **zero ambient permissions**.
* **CEL Capability Gates:** When the AI decides to call a FIDL method (e.g., `transfer_money` or `delete_record`), the request passes through the target service’s **CEL bind policy**.
* **Hardware-Secured Confirmation (future Trusty ConfirmationUI):** If an
  AI-initiated action hits a high-risk policy threshold
  (`policy.requires_user_confirmation`), a future board may let Trusty take
  exclusive ownership of display and input before a hardware-isolated prompt.
  This is intentionally unimplemented until those ownership contracts exist.

## Private Continuous Memory in `redb`

Personal history remains on the device:

* A local **Embeddings Service** listens to system-wide publish/subscribe events over FIDL (files modified, messages received, web pages visited).
* It computes vector embeddings locally and indexes them into an encrypted, per-user `redb` vector partition in the user's encrypted `/data` home directory.
* Because the vector database is stored locally inside the user's encrypted volume, locking the user session or unmounting `/data` instantly cryptographically secures the AI's entire memory.

## Architecture comparison

| Feature | Legacy OS Approach (Mac/Windows/Android) | BexOS Native AI Architecture |
| --- | --- | --- |
| **App Automation** | Screen scraping, OCR, accessibility hacks | **Type-safe FIDL RPC invocation** |
| **Compound Tasks** | Manual context switching across apps | **JIT WASM transient UI synthesis** |
| **Context Perception** | Continuous heavy screenshots / pixel scanning | **Compositor vector scene-graph feed** |
| **Safety & Control** | Probabilistic prompt filters (bypassable) | **Kernel-enforced CEL capability boundaries** |
| **Privacy & Memory** | Cloud vector sync or unencrypted local logs | **Encrypted local `redb` tied to user `U-KEK`** |
