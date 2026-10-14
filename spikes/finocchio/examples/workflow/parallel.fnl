(local cycle (require :cycle))
(local reviewer (require :reviewer))
(local reviser (require :reviser))
(local meta (data.parse "{\"intent\":\"Two unchanged saved workflow functions and ordinary synthesis.\",\"input_schema\":{\"type\":\"array\",\"minItems\":2,\"maxItems\":2},\"output_schema\":{\"type\":\"object\",\"required\":[\"results\",\"approved\"]}}"))
;; Stage replacement is ordinary code. It cannot add host grants.
(fn review-with-validation [input]
  (host.validate :reviewer :input input)
  (reviewer.run input))
(fn synthesize [results]
  (var approved 0)
  (each [_ result (data.iter results)]
    (when (= (data.scalar (data.get result :status)) :approved) (set approved (+ approved 1))))
  (data.pack {:results results :approved approved}))
(fn run [input]
  (synthesize
    (host.concurrent [(fn [] (cycle.with-stages (data.get input 1) review-with-validation reviser.run))
                      (fn [] (cycle.with-stages (data.get input 2) review-with-validation reviser.run))])))
{:meta meta :run run :synthesize synthesize}
