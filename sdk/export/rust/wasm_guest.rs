//! Public bindings for the BexOS portable service lifecycle.
//!
//! Implement [`exports::bexos::wasm::lifecycle::Guest`] and invoke
//! [`export!`] to produce a component accepted by the BexOS WASM runner.

wit_bindgen::generate!({
    inline: r#"
        package bexos:wasm@1.0.0;

        interface lifecycle {
          dispatch: func(resource-id: u32);
          checkpoint: func() -> list<u8>;
          restore: func(checkpoint: list<u8>) -> result;
          activate: func();
          abort: func();
        }

        world service {
          export lifecycle;
        }
    "#,
    world: "bexos:wasm/service",
    pub_export_macro: true,
    generate_all,
});
