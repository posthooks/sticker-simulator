* If we get an error in generated code, tell the user a command to start
  investigation.
* Try using a workspace instead of setting target directory, copying Cargo.lock
  etc.
* Consider adding a crate to aid in interfacing with Evcxr.
* Compile item-only crates as rlibs instead of dylibs to avoid having them get
  recompiled next line.
* Tab completion. Perhaps bring up RLS and query it to determine completion options.
* Allow history of session to be written as a crate.
* Allow history