(local {: run} (require :local/adapter))
(fn [input]
  (due.concurrent :components [(fn [] (run :a)) (fn [] (run :b))]))
