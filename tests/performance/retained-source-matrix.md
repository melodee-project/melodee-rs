# Retained-source mixed-load harness

`scripts/perf_retained_source_matrix.py` must run as the direct child of the
owned disposable fixture runner `scripts/perf_representative.py`. It creates a
random private ownership schema and one-row marker table in that fixture;
otherwise it does not alter the source catalog. It clones the fixture
sequentially and runs the production server's retained reobserver with generated
WAV input and loopback HTTP/SQL probes. The obligations are seeded directly:
this does not benchmark original ingestion, copy, or receipt binding. Result
files describe the measured scope and sampling limitations. Changed sources
are held for future copy-only intake, not turned into new work items; the
workload measures this temporary safety hold, not the unavailable intake path.
The configured
benchmark administrator must be able to query PostgreSQL's
`pg_control_system()`; inability to establish server identity stops the run.

For every candidate the harness first checks that the name is absent, then
writes and fsyncs a unique ownership manifest inside its newly created private
0700 output directory **before** attempting `CREATE DATABASE`. The clone must
contain the exact random marker inherited from the owned template. Cleanup
independently verifies the configured endpoint, PostgreSQL system identifier,
primary status, actual candidate database name, marker, and database OID before
issuing `DROP DATABASE ... WITH (FORCE)`. It refuses to drop an existing or
inaccessible candidate without that proof. This protocol assumes no other
privileged actor concurrently replaces a candidate between verification and
DROP; PostgreSQL does not offer an atomic marker-conditional DROP. Run only
against a disposable, exclusively controlled PostgreSQL instance. PostgreSQL
client environment overrides are removed for these operations; pass connection
settings via the explicitly supplied URI.

SIGINT and SIGTERM request cleanup, but SIGKILL, process or host failure, and
connection failure can interrupt cleanup. An unacknowledged CREATE with an
absent candidate also leaves the manifest: a server operation may still be
finishing. Preserve the private output directory and its `*.ownership.json`
file whenever cleanup is unresolved. After stopping and verifying any owned
orphan server processes and resolving the database connection, an operator can
run:

```text
python3 scripts/perf_owned_clone.py /path/to/private/output/<name>.ownership.json
```

Set `MELODEE_TEST_DATABASE_URL` in the environment to the same administrator
endpoint; do not put credentials in the manifest or retained evidence. The
recovery command leaves the manifest for operator audit and refuses mismatched
or inaccessible databases. An `absent` result is a point-in-time observation;
for an uncertain CREATE, check again after confirming the original database
backend is gone. The operator must separately inspect and remove only known
owned orphan processes, temporary media, and the private manifest directory.
The recovery tool does not discover or terminate those resources. Do not drop a
database solely because its name resembles a fixture name.
