;; Only this in-memory module is available to import-macros.
{:defn (fn [name params doc body ...]
         (assert (= (select "#" ...) 0) "defn expects one body expression")
         (assert (not= body nil) "defn requires a body")
         (assert (sym? name) "defn expects a name")
         ;; The generated reference is deliberately global. Fail closed when a
         ;; surrounding local/parameter could capture it; _G is capturable too.
         (assert-compile (not (in-scope? (sym :contract)))
                         "defn requires unshadowed global contract" name)
         (assert-compile (not (in-scope? (sym :_ENV)))
                         "defn requires unshadowed contract environment (_ENV)" name)
         (assert (and (not= (tostring name) :contract) (not= (tostring name) :_ENV))
                 "defn reserves the names contract and _ENV")
         (assert (and (sequence? params) (= 0 (% (# params) 2)))
                 "defn expects [name :type ...]")
         (assert (= (type doc) :string) "defn expects documentation")
         (local args (sequence))
         (local specs (sequence))
         (for [i 1 (# params) 2]
           (assert (sym? (. params i)) "parameter must be a symbol")
           (assert (and (not= (tostring (. params i)) :contract)
                        (not= (tostring (. params i)) :_ENV))
                   "defn reserves the parameters contract and _ENV")
           (assert (= (type (. params (+ i 1))) :string) "contract type must be a string")
           (table.insert args (. params i))
           (table.insert specs {:name (tostring (. params i)) :type (. params (+ i 1))}))
         ;; Recursive calls resolve to the public wrapper, so they are checked too.
         `(local ,name
            (contract.define ,(tostring name) ,specs ,doc
              (fn [,name] (fn ,args ,body)))))
 :twice (fn [value ...]
          (assert (and (not= value nil) (= (select "#" ...) 0)) "twice expects one expression")
          (local tmp (gensym "once"))
          `(let [,tmp ,value] (+ ,tmp ,tmp)))}
