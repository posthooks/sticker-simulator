# How it works

* We parse the supplied string using the syn crate in order to split the code
  into separate statements. How it does this is pretty gross since unfortunately
  the syn AST won't currently give us spans we can use, but hopefully that'll be
  resolved eventually. This is done in statement\_splitter.rs.

* We also use syn to identify the type of each statement, be it an item
  (function, struct, enum etc) or a statement or expression.
  
* If we've got a statement or expression, we wrap it in a generated function
  body with a unique name. Extra code added to the function is responsible for
  saving and restoring variables between executions, handling panics etc.
  
* We write the code as a crate then get cargo to build it and write the result
  as a shared object (e.g. a .so file on Linux).
  
* We open the shared object (e.g. using dlopen on Linux), look up the s