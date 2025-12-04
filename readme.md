# labman

**labman** is an open-source, operator-friendly manager for running one or more
local LLM endpoints as a single unified “AI node” for a distributed inference
network.

It sits between:

- **Portman** (the local daemon that runs your actual LLM workloads)
- **Conplane** (the remote control plane that schedules and directs work across the network)

and acts as a **secure WebSocket proxy** that forwards structured protocol messages between them according to the shared messaging spec in `protocol.md`.

It is designed for homelab GPU owners, ex-miners, and small operators who want
to contribute compute without exposing their machines directly to the public
internet.

`labmand` provides:

- A single process that runs on one machine in your network  
- A secure post-quantum WireGuard tunnel (via Rosenpass) back to the control plane  
- A unified OpenAI-compatible API exposed over that tunnel
- Automatic discovery of your local LLM servers (Ollama, vLLM, llama.cpp, mistral.rs, etc.)  
- Local health checking and capability reporting  
- Registration + heartbeat to the control-plane  
- A **WebSocket proxy** that speaks the Portman ⇄ Conplane messaging protocol defined in `protocol.md`  
- Zero inbound ports required on your network  
- Full transparency: all code is open-source and auditable  

Operators stay in control of their hardware.  
The control-plane sees your environment only through a single, minimal, WG-protected interface.

---

## Why labman?

Typical distributed AI networks require:
- Running multiple agents  
- Exposing each GPU server directly to the internet  
- Complicated NAT and port forwarding  
- Invasive control-plane agents running on multiple nodes  
- Custom, ad-hoc protocols between local daemons and the control plane  

**labman solves all of that with a single daemon**:

- One post-quantum WireGuard tunnel (Rosenpass)
- One secure internal API
- One config file
- Multiple local endpoints
- No local network exposure  
- No root-level remote control features  
- No model execution inside the agent — your LLM servers do that

labman simply **manages**, **proxies**, and **reports**:

- It terminates a WebSocket connection from **Portman** on the LAN side.
- It maintains a WebSocket connection to **Conplane** over the WireGuard tunnel.
- It forwards and validates structured messages (registration, heartbeats, directives, metrics, etc.) according to the shared protocol contract in `protocol.md`.

---

## Features

- 🟢 **Open-source and fully auditable**
- 🔒 **End-to-end encrypted post-quantum WireGuard tunnel (Rosenpass)**
- 🧰 **Supports multiple local LLM endpoints**
- 🔌 **Works with any OpenAI-compatible server**
- 🎛️ **Model-aware routing driven by the control plane**
- 🌡️ **Endpoint health checks**
- 💬 **Unified OpenAI API served over the tunnel**
- 🔁 **WebSocket proxy that implements the Portman ⇄ Conplane messaging protocol**  
- 🫀 **Automatic control-plane heartbeat and agent registration via protocol messages**
- 🧪 **Homelab-first design**
- 🐧 **Ships with a systemd service file**

---

## High-Level Flow

```

```
     ┌────────────────────┐
     │     Conplane       │
     │  (control plane)   │
     └───────▲────────────┘
             │ Post-Quantum WireGuard
             │ tunnel (Rosenpass)
     ┌───────┴────────────┐
     │     labmand         │
     │   (this project)    │
     └───────┬────────────┘
             │ WS (Portman ⇄ labman)
  local LAN  │
             │ resolves opaque model slugs to local endpoints
```

┌─────────────┼───────────────┐
│             │               │
┌───────┐    ┌───────┐      ┌────────┐
│Portman│    │vLLM   │      │llama.cpp│
│daemon │    │Ollama │      │  etc.   │
└───┬───┘    └───────┘      └────────┘
    │  WS protocol (RegisterAgent, Heartbeat, Directives, Metrics, etc.)
    ▼
  labmand WS proxy

```

labmand exposes a single OpenAI-compatible API to Conplane **and** a WebSocket endpoint for Portman on the LAN side.  
Internally it:

- terminates the Portman WebSocket connection,
- validates and forwards messages defined in `protocol.md` between Portman and Conplane,
- resolves opaque model identifiers (slugs) provided by Conplane into concrete local endpoints and model IDs,
- and then proxies requests accordingly to your local runtimes.

---

## Project Structure

```

labman/
bin/labman-daemon/      # main daemon (systemd-friendly)
bin/labman-cli/         # operator tools
crates/
labman-core/            # shared types and errors
labman-config/          # config loading
labman-wireguard/       # Post-quantum WG via Rosenpass (native Rust)
labman-endpoints/       # local endpoint management
labman-proxy/           # OpenAI proxy served over WG
labman-control/         # control-plane client (HTTP/WS to Conplane)
labman-telemetry/       # logging + tracing setup
# future / in-progress:
# labman-ws-portman     # Portman-facing WS server implementing protocol.md
docs/
architecture.md
protocol.md             # Portman ⇄ labman ⇄ Conplane messaging protocol

````

---

## Quick Start (for operators)

1. Install `labmand` (binary or container).
2. Drop a config file at `/etc/labman/labman.toml`.
3. Start the daemon:

```bash
sudo systemctl enable --now labmand
````

4. labman will:

   * bring up post-quantum WireGuard tunnel (Rosenpass)
   * register your node
   * discover your local endpoints
   * start proxying over WG

---

## License

MIT
