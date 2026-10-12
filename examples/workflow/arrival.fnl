;; Manifest v2: loading returns a function; all job effects occur on invocation.
(local p (require :patterns/v1))
(fn [input]
  (local a (due.start (p.job :correctness :reader
                           (.. "Review correctness: " input.task))))
  (local b (due.start (p.job :edges :reader
                           (.. "Review edge cases: " input.task))))
  (local first (due.wait_any [a b]))
  (local remaining (if (= first.id :correctness) b a))
  (local prompt (if first.ok
                   (.. "Examine the first review: " (json.encode first.answer))
                   "The first review failed. Independently inspect the task."))
  (local replacement (due.start (p.job :replacement :reader prompt)))
  (local stopped (due.cancel remaining))
  {:first first :cancelled stopped :replacement (due.wait replacement)})
