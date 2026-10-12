;; Ordinary functions with lexical captures; no macro-only workflow API.
(fn in-phase [name operation]
  (fn [...]
    (due.phase name)
    (operation ...)))
{:in_phase in-phase}
