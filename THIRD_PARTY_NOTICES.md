# Third-party notices

## Temporal Server

The knowledge packs in `knowledge/temporal/` quote [Temporal Server](https://github.com/temporalio/temporal):
the 1.31 pack its releases from v1.27.0 to v1.31.4, and the 1.32 pack v1.32.0 and v1.32.1.
Each pack's `generated/dynamic-config.toml` holds the key, default and description of every
dynamic configuration setting, which `gen-dc-claims` extracts from the Temporal source, and its
`generated/server-metrics.toml` holds the name and description of every server metric, which
`gen-metric-claims` extracts the same way. Other claims quote short excerpts as provenance.
Kyumei contains no Temporal code.

```text
The MIT License

Copyright (c) 2025 Temporal Technologies Inc.  All rights reserved.

Copyright (c) 2020 Uber Technologies, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

## Other quoted sources

Each pack's `generated/sdk-metrics.toml` holds the name of every metric the
Temporal SDKs define, with the descriptions the Rust SDK (Core) gives, which
`gen-sdk-metric-claims` extracts from:

- the Rust SDK and Core, as forked at [tokeira/sdk-rust](https://github.com/tokeira/sdk-rust)
  `acac9e3f` (MIT);
- the [Go SDK](https://github.com/temporalio/sdk-go) (MIT);
- the [Java SDK](https://github.com/temporalio/sdk-java) (Apache-2.0).

Claims about the Temporal SDKs, the Temporal API and the Temporal CLI quote short excerpts
from their sources as provenance. So do the claims about how metrics are exported and how Go's
`database/sql` waits for a pooled connection. They quote:

- the [Temporal API](https://github.com/temporalio/api-go) Go bindings (MIT);
- the SDKs above, and the [Python SDK](https://github.com/temporalio/sdk-python) (MIT);
- the [Temporal CLI](https://github.com/temporalio/cli), v1.7.0 to v1.9.1 (MIT);
- [uber-go/tally](https://github.com/uber-go/tally) v4.1.17 (MIT);
- the [OpenTelemetry Go](https://github.com/open-telemetry/opentelemetry-go) Prometheus
  exporter v0.56.0 (Apache-2.0);
- the Go SDK's tally and OpenTelemetry handlers (MIT);
- [Micrometer](https://github.com/micrometer-metrics/micrometer) 1.9.9 and 1.13.6
  (Apache-2.0);
- the [Prometheus Java client](https://github.com/prometheus/client_java) 1.2.1
  (Apache-2.0);
- the [Go standard library](https://go.dev/) (BSD-3-Clause).

Each source is named, with its repository and revision, in each pack's
`sources/sources.toml`.

## tempdes

`crates/kyumei-extract/src/registry_gen.rs` and `crates/kyumei-extract/src/registry.rs` are
copied from tempdes (`src/config/registry_gen.rs` and `src/config/dynamic.rs` at `848fa47`).
Copyright (c) 2026 The tempdes contributors. tempdes is licensed under MIT OR Apache-2.0;
Kyumei uses this code under the Apache License, Version 2.0.

## Temporal server operations

`knowledge/temporal/1.31/references/operations.toml` cites the alert runbooks,
playbooks and Grafana dashboards of
[tsurdilo/temporal-server-operations](https://github.com/tsurdilo/temporal-server-operations)
at commit `5a71c3656cc0bca831b61142b64c3672cc974b1b`. The repository has no
licence, so Kyumei includes none of its content: each reference gives the path,
the section heading or the dashboard uid and panel id, and the title, to cite
it, and links to it. Kyumei reads dashboards only from a copy the operator
supplies. Besides titles and headings, a reference quotes the line that declares
a version (a dashboard's title, or a row of the playbooks index), to cite it.

## Time Zone Database

Kyumei's binary carries the [IANA Time Zone Database](https://www.iana.org/time-zones),
through the `jiff-tzdb` crate, to show times in the operator's zone. The
database is in the public domain.

