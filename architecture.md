# labman Architecture & Design

This document explains **how labman works internally**, its trust and security
model, and the architecture choices behind the project.

It is intended for: operators who want to configure
- what infrastructure is available
- what resources and capacity are available
- what utilisation looks like at any given time

---

# 1. Overview

**labman** is a homelab-friendly agent that turns multiple local LLM runtime
servers into a single “node” in a distributed compute network.

Its goals:

- Provide a *single secure ingress* (via WireGuard)
- Aggregate multiple local OpenAI-compatible inference endpoints
- Expose them as a unified OpenAI API to a remote control-plane
- Terminate a local WebSocket connection from Portman and a remote WebSocket connection to Conplane, forwarding structured protocol messages between them
- Maintain a minimal, auditable footprint
- Avoid requiring any public exposure of the operator’s network

labman does *not* execute models itself — it simply discovers, schedules across,
and proxies to local LLM endpoints.

---


# 2. High-Level Components

labman is composed of several crates:

````
labman-core             Shared types, errors, model descriptors
labman-config           Loads and validates ./labman.toml
labman-wireguard        Manages WG config/bootstrap
labman-endpoints        Registry of local endpoints + health checking
labman-proxy            OpenAI proxy exposed over WG
labman-client           Client for the control-plane API (HTTP/WS to Conplane)
labman-telemetry        Logging and tracing setup
labman-ws-portman       Portman-facing WS server implementing protocol.md (planned / in-progress)
labman-daemon (labmand) Daemon combining all components
labman-cli (labman)     Command-line interface for labman
```

Each crate has a single responsibility and a minimal API boundary.

---

# 3. Data Flow

## 3.1. Control-plane onboarding

1. Operator installs `labmand`.
2. Operator obtains node token + Rosenpass/WireGuard config from control-plane.
3. labman:
   - validates config
   - brings up post-quantum WireGuard interface (via Rosenpass)
   - registers itself using node token & minimal metadata
   - establishes a **persistent WebSocket connection to Conplane**, using the envelope format and message types defined in `protocol.md`

The control-plane now knows:
- node identity
- supported local models
- health/latency estimates
- WG endpoint address for routing inference

All of this happens **outbound** from the operator environment.

There are **zero local inbound ports** required.

---


## 3.2. Inference flow

### Step 1 — control-plane → labmand (over WireGuard)
The distributed inference gateway sends a standard OpenAI-compatible request to:

```

http://<wg-ip-of-node>:8080/v1/chat/completions

```

### Step 2 — control-plane selects an endpoint and model slug
The distributed inference gateway (control-plane) decides **which tenant, which endpoint, and which concrete model** should serve the request based on:

- historical latency and reliability per node/endpoint  
- reported capabilities and health from labman  
- its own accounting and scheduling policies  

It then encodes the chosen `(tenant, endpoint_slug, model_id)` triple into an opaque **model slug** and sends a standard OpenAI-compatible request where the `model` field contains this slug.

labmand does **not** make global endpoint/model selection decisions; it only resolves the opaque `model` slug into a specific local endpoint and concrete model identifier.

### Step 3 — local proxy → endpoint
labmand:

1. Treats the incoming OpenAI `model` field as an **opaque slug** supplied by the control-plane.
2. Resolves this slug into a `(tenant, endpoint_name, model_id)` triple using its local registry:
   - `tenant` is used for attribution/compensation and observability.
   - `endpoint_name` identifies a specific local endpoint.
   - `model_id` is the actual model string that endpoint understands (e.g. `mistral-nemo:12b`).
3. Rewrites the upstream request so that the endpoint sees the concrete `model_id` instead of the slug.
4. Forwards the OpenAI request to the selected local endpoint:

```

POST http://127.0.0.1:11434/v1/chat/completions

```

It then streams the response back to the control-plane.

### Step 4 — control-plane → client
The control-plane aggregates usage, secures billing, and returns output to the final user.

---

# 4. Security Model

labman is designed so that operators can safely participate in a distributed inference network **without exposing their LAN**, and without granting the control plane any visibility or reach beyond a single, sandboxed WireGuard interface.

This section documents the security model, trust boundaries, and invariants enforced by labman.

---

## 4.1 Threat Model

labman assumes:

- The **control plane** is not necessarily trusted by the operator.
- The operator may already have a carefully designed:
  - firewall,
  - routing/forwarding configuration,
  - other WireGuard or VPN interfaces.
- labman must not:
  - expose new paths into the LAN,
  - turn the host into a router for the control plane,
  - change global networking behaviour outside its own interface.

labman therefore treats the WireGuard connection as a **narrow, single-purpose transport channel** and keeps its impact on the host network as small and explicit as possible.

---


## 4.2 WireGuard Interface Isolation

labman creates and manages a **dedicated post-quantum WireGuard interface** (via Rosenpass), default name:

```
labman0
```

or an operator-specified override.

Security properties:

* The interface is assigned a **single /32 address** (e.g. `10.90.X.Y/32`).
* No LAN addresses (e.g. `192.168.*`) are ever assigned to labman0.
* labman never modifies system-wide routing or forwarding settings.
* labman never enables IPv4/IPv6 forwarding.
* labman never adds NAT/masquerade rules.

This ensures that labman0 cannot act as a router or gateway to the operator’s LAN.

---

## 4.3 No LAN Exposure Over WireGuard

labman implements a strict rule:

> **Nothing from the operator’s LAN is reachable through the WireGuard tunnel.**
> Only labman’s own OpenAI-compatible proxy is exposed.

Control-plane traffic flows as:

```
control-plane → (WireGuard) → labman → local LLM endpoints (via HTTP)
```

Local LLM endpoints remain reachable **only on the operator’s machine or LAN**, and are never mapped into the WireGuard network.

Thus:

* The control plane can only reach the single synthetic WG IP assigned to labman (`10.90.X.Y`).
* It cannot address any LAN host (`192.168.*`, `10.*.*.*`, etc.).
* It cannot enumerate or probe local infrastructure.

---

## 4.4 Handling Control-Plane Misconfiguration

WireGuard’s `AllowedIPs` parameter is a common source of confusion. It is both a routing rule and an ACL, but **does not grant the remote peer access to those subnets unless the local system forwards traffic to them**.

labman enforces the following invariants to protect against misconfiguration or malicious configuration on the control-plane side:

* Even if the control plane erroneously sets `AllowedIPs=192.168.0.0/24`,
  the operator’s machine does **not** route this traffic into the LAN.
* Packets destined for LAN addresses arriving via labman0 have **no matching route** and are dropped by the kernel.
* IP forwarding remains disabled.
* Firewall policies (optional but recommended) block all forwarding from `labman0` to any other interface.

This guarantees that the operator’s LAN is not reachable or exposed, regardless of control-plane configuration.

---

## 4.5 labman Proxy as the Sole Exposure

The **only** service reachable over the WireGuard interface is labman’s internal OpenAI-compatible proxy, bound explicitly to the WireGuard address:

```
listen_addr = "10.90.X.Y:8080"
```

labman never binds to:

* `0.0.0.0`
* public IPs
* LAN interfaces

Only authenticated traffic over the WG tunnel can reach this API.

All local inference calls are proxied internally:

```
labman → http://127.0.0.1:<port>/v1/...  
labman → http://LAN-IP:<port>/v1/...
```

The control plane never sees these addresses.

---


## 4.6 No Remote Execution or System-Level Control

labman avoids the security pitfalls of typical “node agents”:

* No remote command execution.
* No file transfer or file system access from the control plane.
* No ability for the control plane to modify operator config.
* No port scanning, probing, or LAN discovery.
* No resource management or GPU control.

Its sole responsibilities are:

* maintain a WireGuard tunnel,
* serve an OpenAI-compatible proxy over it,
* route traffic to locally configured LLM endpoints,
* and send a minimal heartbeat to the control plane.

---

## 4.7 Operator Transparency and Auditability

Because labman is open-source:

* Operators can inspect exactly how networking and WireGuard are configured.
* Changes to interface setup or API binding are visible in the code.
* labman’s scope on the system is clear and limited:
  * The WireGuard interface
  * The listening socket
  * outbound HTTP to configured local endpoints and the control plane

labman is designed to be a **non-invasive component**: it does its job without weakening or second-guessing an operator's existing security posture.

---


## 4.8 Post-Quantum Cryptography with Rosenpass

labman implements **post-quantum secure WireGuard** using Rosenpass, ensuring that the encrypted tunnel between the operator's node and the control plane remains secure even against future quantum computer attacks.

### Why Post-Quantum?

Traditional public-key cryptography (including WireGuard's Curve25519) is vulnerable to attacks from sufficiently powerful quantum computers. While such computers don't exist today, encrypted traffic captured now could be decrypted in the future when quantum computers become available — a "harvest now, decrypt later" attack.

For a distributed inference network handling potentially sensitive workloads, this is unacceptable. Operators need assurance that their traffic is protected not just today, but for years to come.

### What is Rosenpass?

Rosenpass is an open-source, formally verified post-quantum key exchange protocol built on top of WireGuard. It:

- Uses post-quantum-secure cryptographic primitives (Classic McEliece, Kyber)
- Provides forward secrecy with quantum-resistant guarantees
- Integrates seamlessly with WireGuard's existing tunnel infrastructure
- Adds post-quantum protection without sacrificing WireGuard's performance or simplicity

### labman's Implementation Approach

labman integrates Rosenpass **natively in Rust** using the official Rosenpass libraries:

- **No shell wrappers** — direct Rust API integration for reliability and auditability
- **No external tools** — all cryptographic operations handled in-process
- **Transparent operation** — operators see a single WireGuard interface (`labman0`)
- **Automatic key rotation** — Rosenpass handles post-quantum key exchange and periodic rotation
- **Minimal configuration** — operators provide initial keys; labman handles the rest

The control plane provides the initial Rosenpass configuration (public keys, endpoints) alongside the WireGuard config. labman then:

1. Initializes the Rosenpass key exchange
2. Creates the WireGuard interface with Rosenpass-derived keys
3. Maintains the tunnel, automatically rotating keys as needed
4. Ensures all traffic remains quantum-resistant throughout the session

### Security Properties

This implementation ensures:

- **Quantum resistance**: Tunnel is secure against both classical and quantum attacks
- **Forward secrecy**: Compromise of long-term keys doesn't compromise past sessions
- **Auditability**: All cryptographic code is open-source Rust, reviewable by operators
- **Standards-based**: Uses well-studied post-quantum algorithms (NIST candidates)
- **Defense in depth**: Even if WireGuard's classical crypto is broken, Rosenpass provides protection

Operators can confidently participate in the network knowing their traffic is protected against both present and future threats.

---

# 5. Why One Agent Instead of Many?

Many distributed compute systems require agents on *every* GPU box or VM.

labman deliberately **centralises the operator-side integration**:

- One process to install
- One WG connection
- One registration loop
- One heartbeat
- Many local LLM servers referenced by HTTP URLs
- No need for multiple systemd services or per-box daemons
- Multi-box and multi-runner setups are simple to maintain

Operators can point labman to several machines in their LAN:

```toml
[[endpoint]]
name = "vllm-box"
base_url = "http://192.168.1.42:8000/v1"

[[endpoint]]
name = "ollama-box"
base_url = "http://192.168.1.99:11434/v1"
max_concurrent = 8

[[endpoint]]
name = "filtered-endpoint"
base_url = "http://192.168.1.100:8000/v1"
models.include = ["mixtral-*", "qwen*"]   # optional: only proxy these models
models.exclude = ["gpt-4o*", "gpt-3.5-*"] # optional: exclude these models
```

**Model Discovery:** labman queries each endpoint's `/v1/models` API to discover what models are available. There is no need to manually configure model lists.

**Model Filtering (Optional):** The `models.include` and `models.exclude` fields allow operators to restrict which models from an endpoint are exposed through the proxy. These are glob patterns applied as filters on top of the endpoint's advertised models. If unspecified, all models from the endpoint are available.

labman handles everything else.

---

# 6. Internal Architecture

## 6.1. labman-core

Defines:

* errors (`LabmanError`)
* shared config structures
* model descriptors
* endpoint definitions

## 6.2. labman-config

Handles:

* TOML file loading
* minimal validation
* merging operator config with control-plane-supplied config (optional)

## 6.3. labman-wireguard

Implements **post-quantum WireGuard** using Rosenpass natively in Rust.

This crate does **not** use shell wrappers or external `wg-quick` commands. Instead, it directly integrates with the Rosenpass Rust libraries to provide quantum-resistant key exchange on top of WireGuard.

Responsibilities:

* Create and maintain post-quantum WireGuard interface(s) using Rosenpass
* Implement Rosenpass key exchange and rotation
* Manage interface state (up, down, monitoring)
* Bootstrap and validate cryptographic keys
* Handle WireGuard configuration natively via netlink
* Ensure quantum-resistant security posture

This approach aligns with labman's security ethos: auditable, transparent, and implemented correctly without relying on external tools that could introduce vulnerabilities or operational complexity.


## 6.4. labman-endpoints

Manages:

* endpoint registry
* health checks
* model-to-endpoint mapping
* load/concurrency estimation
* selection algorithms

### Model Discovery and Filtering

labman **automatically discovers** available models by querying each endpoint's `/v1/models` API. There is no need for operators to manually configure model lists—this ensures the proxy always reflects reality.

**Discovery Process:**
1. On startup and periodically, labman queries `GET /v1/models` from each configured endpoint
2. Each endpoint returns its list of available models (standard OpenAI format)
3. labman builds an internal registry mapping model names to capable endpoints
4. When a request arrives for a specific model, labman selects an endpoint that advertises it

**Optional Filtering:**

Operators can optionally restrict which models from an endpoint are exposed:

```toml
[[endpoint]]
name = "shared-server"
base_url = "http://192.168.1.50:8000/v1"
models.include = ["llama*", "mistral*"]  # only expose models matching these globs
models.exclude = ["*-uncensored"]        # exclude models matching these globs
```

- `models.include`: If specified, only models matching these glob patterns are proxied
- `models.exclude`: If specified, models matching these patterns are filtered out
- Filters are applied **after** querying `/v1/models`, not instead of it
- If both are specified, include is applied first, then exclude
- If neither is specified, all models from the endpoint are available

This design avoids configuration drift—operators never need to manually sync config files with model availability.

## 6.5. labman-proxy

Exposes:

```
/v1/chat/completions
/v1/completions
/v1/models
```

to the control-plane over WG.

* Receives OpenAI requests
* Selects an endpoint
* Forwards & streams responses
* Emits usage and latency information that can be correlated with protocol-level `UsageReport` / metrics messages

## 6.6. labman-client

Handles secure outbound communication:

* node registration
* capability sync
* periodic heartbeat
* WebSocket client lifecycle for Conplane, including reconnect backoff and authentication (WS transport for the messaging protocol)

Planned future support:

* node mode changes (e.g., maintenance mode)
* future protocol extension handling (new message kinds negotiated via `protocol.md`)

## 6.7. labman-daemon (labmand binary)

Co-ordinates everything:

* initializes logging
* loads config
* brings up WG
* spawns heartbeat/registration tasks
* launches the proxy server
* starts the Portman-facing WebSocket server and the Conplane-facing WebSocket client
* wires the **message routing layer** that implements the envelope format and message types from `protocol.md` (RegisterAgent, Heartbeat, Metrics, Directives, Ack/Error, etc.)

---




# 7. Extensibility

labman is designed to evolve:

* new endpoint types (GPU clusters, Kubernetes pods, DPUs)
* benchmark-based model scoring
* dynamic cost-aware routing
* richer node metadata (energy use, temperature, VRAM load)
* custom user-supplied routing plugins
* integration with homelab dashboard tools

All via cleanly-separated crates.

---

# 8. Operator Experience

The operator sees:

* one config file (`/etc/labman/labman.toml`)
* one systemd service (`labmand`)
* WG interface (`labman0`)

Everything inside is optional and transparent:

* They choose which endpoints to expose
* They choose which models to advertise
* They choose how much concurrency to allow

The control-plane cannot reach into their LAN — it only sees the endpoints that labman exposes.

---

# 9. Summary

labman provides:

* A **secure**, **open-source**, **minimal** agent
* For homelab GPU operators who want to join a distributed AI network
* Without sacrificing network hygiene, privacy, or autonomy

It prioritises:

* clear boundaries
* simple onboarding
* strong cryptographic isolation
* zero inbound exposure
* auditability
* easy configuration

labman’s purpose is not to run models — it is to **manage**, **aggregate**, and **bridge** them to the network safely, while implementing the Portman ⇄ labman ⇄ Conplane messaging protocol defined in `protocol.md` via a pair of WebSocket connections and a strict, auditable routing layer.
