# labman Agent & Runtime Conventions

This document captures conventions and preferences for how labman components
(daemons, CLIs, background agents) should behave and how we expect contributors
to design their interfaces.

The goal is to make labman easy to operate in realistic environments while
keeping behavior predictable and explicit.

---

## 1. Configuration and Environment Variables

### 1.1 No labman-specific env vars

labman **must not introduce its own environment variables** for configuration or
behavioral control.

- Do **not** add `LABMAN_*` (or similar) environment variables to:
  - Select configuration files
  - Override configuration values
  - Control operational behavior (e.g., feature toggles, log levels, modes)

The only acceptable uses of environment variables are:

- For third-party components or tools that labman integrates with and which
  *already* rely on env vars (e.g., `RUST_LOG`, `HTTP_PROXY`).
- For development tooling, testing harnesses, or CI infrastructure that is
  *external* to the deployed labman binaries.

**Rationale:**

- Env vars are hard to audit and reason about in production.
- They are invisible in many debugging contexts (systemd units, Kubernetes
  manifests, etc. might override them in opaque ways).
- Keeping configuration in explicit files and CLI flags leads to more
  reproducible deployments.

### 1.2 Configuration discovery

Given the ban on labman-specific env vars, configuration discovery should use
deterministic and explicit mechanisms:

- **Configuration file locations** (example for `labmand`):
  1. Path explicitly passed as a CLI flag (see section 2).
  2. Well-known default locations (e.g., `/etc/labman/labman.toml`,
     `./labman.toml`), in clearly documented order.

- **Priority rule**:
  - An explicitly provided path via CLI flag **always wins**.
  - If not provided, probe known paths in deterministic order.
  - If no configuration file is found, fail fast with a clear error message and
    actionable guidance.

### 1.3 Overriding configuration

When overrides are required:

- Prefer **configuration file edits**.
- If runtime overrides are necessary (for example, for quick testing):
  - Use **explicit CLI flags** (long form preferred in docs, see below).
  - Document exactly how CLI flags interact with config file values.

---

## 2. Command-line Interface Conventions

All labman binaries (e.g. `labmand`, `labman`) should follow consistent CLI
behavior.

### 2.1 Long vs short flags

**Preference:**

- Documentation, examples, and sample unit files must use **long-form flags**.
- Short-form flags are supported and documented, but not used as the primary
  form in narrative documentation.

**Examples (preferred in docs):**

- `--config /etc/labman/labman.toml`
- `--log-level info`
- `--endpoint-timeout 30`

**Examples (allowed, for keyboard-heavy usage):**

- `-c /etc/labman/labman.toml`
- `-L info`
- `-t 30`

**Rules:**

1. Every meaningful option **must** have a long-form name:
   - Use `kebab-case`: `--control-plane-url`, `--listen-port`, etc.
2. When defining short-form aliases:
   - Use single-character, mnemonic where possible: `-c` for `--config`,
     `-p` for `--port`.
   - Avoid collisions; if conflicts arise, prefer clear long-form and drop or
     change the short form.

### 2.2 Documentation style

When writing docs, examples, or comments:

- **Always show the long-form flag** in primary examples.
- Where relevant, also mention the short form once, e.g.:

  > You can specify the config file with `--config PATH` (short: `-c PATH`).

- Do **not** use short-form only in any public-facing documentation. The reader
  should never be forced to look up what `-c` means.

### 2.3 Exit behavior and errors

- On invalid flags or arguments:
  - Print a concise error.
  - Show or reference `--help`.
  - Exit with a non-zero status code.
- On configuration errors:
  - Describe the problem clearly, including the path and the field where
    possible.
  - Avoid silently falling back to defaults in surprising ways.

---

## 3. Logging and Telemetry

### 3.1 Log level control

- Primary control should be via:
  - Configuration file (e.g., `[telemetry] log_level = "info"`).
  - CLI flags (e.g., `--log-level info`).

- Environment-based mechanisms (e.g. `RUST_LOG`) are allowed only because the
  Rust ecosystem relies on them, but:
  - They should be treated as a **secondary**/legacy mechanism.
  - Our docs should emphasize config/flags first.

### 3.2 Format

- Support for text and JSON logging is encouraged.
- Configuration of log format should be via config file and/or CLI flags, not
  env vars.

---

## 4. Agents and Daemon Design

### 4.1 Determinism and explicitness

Agents/daemons should:

- Be explicit about:
  - Which config file was loaded.
  - Any defaults that were applied.
- Avoid hidden behavior controlled by:
  - Environment variables (labman-specific).
  - Implicit global state.

### 4.2 Systemd / service units

When providing example units:

- Use **long-form flags** exclusively:
  - Example: `ExecStart=/usr/bin/labmand --config /etc/labman/labman.toml`
- Do not rely on environment variables for core behavior:
  - Avoid `Environment=LABMAN_CONFIG=...` or similar.

If integration with external systems requires env vars (for example, a third
party metrics exporter), document those as **integration-specific**, not as
core labman behavior.

---

## 5. Contributor Guidelines

When you add or modify behavior for any labman binary:

1. **Do not** add new environment variables for labman behavior.
   - If you feel an env var is necessary, discuss it first; it is likely we
     should use configuration files or explicit flags instead.

2. **Always** define long-form flags for new CLI options.
   - Use descriptive, kebab-case names.
   - Add short forms only where they provide clear ergonomic benefit.

3. **In docs and examples**:
   - Prefer long-form flags.
   - Mention short forms parenthetically as aliases.

4. **In configuration handling**:
   - Make the precedence explicit:
     1. CLI flags
     2. Configuration file(s)
     3. Hard-coded defaults (documented)

5. **In error messages**:
   - Surface enough context (path, key, expected vs actual) to be actionable.
   - Avoid vague messages that force the operator to read source code.

Adhering to these conventions will keep labman’s operational surface coherent
and predictable across different environments and deployment styles.