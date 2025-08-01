[package]
name = "js_runtime"
authors.workspace = true
edition.workspace = true
# edition = "2024"
rust-version.workspace = true
repository.workspace = true

[lib]
path = "src/lib.rs"

{% if prompts['use-tracing'] %}
[features]
default = ["enable-tracing"]
enable-tracing = ["trace/enabled"]

[dependencies]
trace = { workspace = true, optional = true }
{% endif %}