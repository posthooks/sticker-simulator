# How it works

* We parse the supplied string using the syn crate in order to split the code
  into separate statements. How it does this is pretty gross since unfortunately
  the syn AST won't currently give us spans we can use, but hopefully that'll be
  resolved eventually. This is done in statement\_splitter.rs.

* We also use syn to identify the type of each statement, be it an item
  (function, struct, enum etc) or a statement or expression.
  
* If we've got a statement or expression, we wr