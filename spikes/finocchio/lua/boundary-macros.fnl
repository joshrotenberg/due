;; Opt-in library experiment. The caller supplies the runtime builder value.
;; This does not resolve a generated free reference at macro definition time.
{:defn-with (fn [builder name params doc body ...]
              (assert (= (select "#" ...) 0) "defn-with expects one body expression")
              (assert (and (not= builder nil) (not= body nil)) "builder and body required")
              (assert (and (sym? name) (not (multi-sym? name))) "simple name required")
              (assert (and (sequence? params) (= 0 (% (# params) 2)))
                      "defn-with expects [name :type ...]")
              (assert (= (type doc) :string) "documentation required")
              (local args (sequence))
              (local specs (sequence))
              (each [i param (ipairs params)]
                (when (= 1 (% i 2))
                  (assert (and (sym? param) (not (multi-sym? param))) "simple parameter required")
                  (assert (= (type (. params (+ i 1))) :string) "literal contract type required")
                  (table.insert args param)
                  (table.insert specs {:name (tostring param) :type (. params (+ i 1))})))
              (local helper (gensym :builder))
              `(local ,name
                 (let [,helper ,builder]
                   (,helper ,(tostring name) ,specs ,doc
                     (fn [,name] (fn ,args ,body))))))}
