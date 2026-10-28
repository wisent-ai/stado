# Journey configuration

`empty.json` is the Stado config a journey selects with `STADO_CONFIG` when
the stado it runs must read no operator configuration. Stado refuses a
`STADO_CONFIG` that names a file that does not exist, so a journey selects
this file instead of a path it never writes. It declares nothing; every
value the journey needs comes from the environment it sets.
