# Capture Scenarios

Scenarios are declarative requests used against a controlled legacy server and later against the Rust
server. Separate public and authenticated files so credentials can be injected only through
environment variables. Scenario files are safe to commit; capture output remains ignored until it is
reviewed and promoted as sanitized golden evidence.
