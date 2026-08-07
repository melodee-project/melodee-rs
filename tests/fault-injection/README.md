# Fault-Injection Tests

Required cases include process termination after each durable transition; expired/stolen worker lease;
source change during settling; destination/database/search outage; cross-filesystem copy interruption;
FFmpeg/fpcalc timeout/crash; permission and disk-full errors; duplicate delivery; and retry after partial
publication. The expected outcome is an explainable durable state and safe next action.
