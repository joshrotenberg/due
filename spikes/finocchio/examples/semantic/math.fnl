(import-macros {: defn : twice} :finocchio)
;; bump in this comment is not a reference. Neither is "bump".
(local label "bump")
(defn bump [x :integer] "Add two using a hygienic macro."
  (+ x (twice 1)))
(fn shadow [bump] (+ bump 1))
{:bump bump :shadow shadow :label label}
