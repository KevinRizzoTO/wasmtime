use anyhow::Result;
use wasmtime::*;

#[test]
fn compiles_with_winch() -> Result<()> {
    let mut c = Config::new();

    // DOIT: once we remove winch as a default feature before push, update the features for this
    // test somehow to include the winch feature.
    c.strategy(Strategy::Winch);

    let engine = Engine::new(&c)?;

    // Winch only supports a very basic function signature for now while it's being developed.
    let test_mod = r#"
    (module
      (func $test (result i32)
        (i32.const 42)
      )
      (export "test" (func $test))
    )
    "#;

    let mut store = Store::new(&engine, ());

    let module = Module::new(&engine, test_mod)?;

    let instance = Instance::new(&mut store, &module, &[])?;

    let f = instance.get_func(&mut store, "test").ok_or(anyhow::anyhow!("test function not found"))?;

    let mut returns = vec![Val::null(); 1];

    // Winch doesn't support calling typed functions at the moment.
    f.call(&mut store, &[], &mut returns)?;

    assert_eq!(returns.len(), 1);
    assert_eq!(returns[0].unwrap_i32(), 42);

    Ok(())
}
