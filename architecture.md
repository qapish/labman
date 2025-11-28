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
labman-client           Client for the control-plane API
labman-telemetry        Logging and tracing setup
labman-daemon (labmand) Daemon combining all components
labman-cli (labman)     Command-line interface for labman
```

Each crate has a single responsibility and a minimal API boundary.

---

# 3. Data Flow

## 3.1. Control-plane onboarding

1. Operator installs `labmand`.
2. Operator obtains node token + WireGuard config from control-plane.
3. labman:
   - validates config
   - brings up WireGuard interface
   - registers itself using node token & minimal metadata

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

### Step 2 — labmand selects an endpoint
labmand reads the model from the request and picks a local endpoint that supports it:

- vLLM  
- Ollama  
- llama.cpp  
- mistral.rs  
- anything OpenAI-compatible  

The decision is based on:
- endpoint model list
- optional concurrency limits
- health history

### Step 3 — local proxy → endpoint
labmand forwards the OpenAI request to the selected local endpoint:

```

POST [http://127.0.0.1:11434/v1/chat/completions](http://127.0.0.1:11434/v1/chat/completions)

````

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

labman creates and manages a **dedicated WireGuard interface**, default name:

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

labman is designed to be a **non-invasive component**: it does its job without weakening or second-guessing an operator’s existing security posture.

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

# Optional policy knobs
max_concurrent = 8
model_include = ["mixtral-*", "qwen*"]   # glob/regex allowed (optional)
model_exclude = ["gpt-4o*", "gpt-3.5-*"] # optional
````

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

Abstracts:

* create and maintain WireGuard interface(s)
* maintain interface state (up, down)
* verifying keys
* bootstrapping config
* userspace WireGuard (no root)
* PQ key bootstrapping (Rosenpass)

## 6.4. labman-endpoints

Manages:

* endpoint registry
* health checks
* model-to-endpoint mapping
* load/concurrency estimation
* selection algorithms

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

## 6.6. labman-client

Handles secure outbound communication:

* node registration
* capability sync
* periodic heartbeat

Planned future support:

* node mode changes (e.g., maintenance mode)

## 6.7. labman-daemon (labmand binary)

Co-ordinates everything:

* initializes logging
* loads config
* brings up WG
* spawns heartbeat/registration tasks
* launches the proxy server

---

# 7. Extensibility

labman is designed to evolve:

* new endpoint types (GPU clusters, Kubernetes pods, DPUs)
* benchmark-based model scoring
* dynamic cost-aware routing
* PQ-enabled transport to control-plane
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

labman’s purpose is not to run models — it is to **manage**, **aggregate**, and **bridge** them to the network safely.
