;; No macros: this exact module also passes Due's existing restricted compiler.
(fn wrapped-wait [handle] (due.wait handle))
(fn run [id prompt]
  (local handle (due.start {:id id :profile :reader :prompt prompt}))
  (wrapped-wait handle))
{: run}
