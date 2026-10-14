;; café: these two calls must not collapse into one expansion.
(macro leaf [x] `(+ ,x 1))
(macro branch [x] `(do (leaf ,x)))
(macro pass [expression] `(do ,expression))
[(branch 10) (branch 20) (pass (leaf 30))]
