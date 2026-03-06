# Security Test Matrix

Maps each security finding to its corresponding automated test.

| Finding ID | Claim | Test | Phase |
|:---|:---|:---|:---|
| mxdx-ji1 | No hardcoded test ports | TuwunelInstance uses port 0 | 3 |
| mxdx-71v | cwd validated against allowlist | test_security_cwd_outside_prefix_is_rejected | 5 |
| mxdx-jjf | argument injection blocked | test_security_git_dash_c_blocked | 5 |
| mxdx-aew | history_visibility=joined | launcher_creates_terminal_dm_on_session_request | 6 |
| mxdx-ccx | zlib bomb rejected | test_security_zlib_bomb_rejected_before_pty_write | 6 |
| mxdx-seq | u64 seq support | seq_counter_supports_u64_range | 6 |
| mxdx-rpl | replay protection | test_security_replayed_event_does_not_double_execute | 8 |
| mxdx-adr2 | double encryption | worker_requests_secret_with_double_encryption | 9 |
| mxdx-tky | test key cfg(test) | test_key_constructor_is_test_only | 9 |
| mxdx-web | CSP headers | csp_header_is_set | 10 |
