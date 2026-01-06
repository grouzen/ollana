# Ollana

## Rust code instructions

- Always collapse if statements per https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if
- Always inline format! args when possible per https://rust-lang.github.io/rust-clippy/master/index.html#uninlined_format_args
- Use method references over closures when possible per https://rust-lang.github.io/rust-clippy/master/index.html#redundant_closure_for_method_calls
- Run `cargo make test` first and if it passes, run `cargo make check-all` automatically after making Rust changes. Do not ask for permission to do this.
- Do not refer to the internal types as `crate::<name>::<symbol>`, import `crate::<name>` instead and call the symbol directly using `<name>::<symbol>`. For example `crate::port_config::ALL_PROVIDER_TYPES` must be `port_config::ALL_PROVIDER_TYPES`. Same applies to such way of importing internal symbols: `ollana::port_config::parse_port_mappings`. 
