(import-macros {: defn-with} :finocchio-boundary)
(local cap contract.define)
(var evaluations 0)
(fn choose [] (set evaluations (+ evaluations 1)) cap)
(fn caller [contract _G builder builder_1_ _ENV]
  (defn-with (choose) factorial [n :integer] "Checked recursion with an explicit builder."
    (if (= n 0) 1 (* n (factorial (- n 1)))))
  (defn-with cap meanings [contract :integer _G :integer builder :integer] "Caller names."
    (+ contract _G builder builder_1_))
  {:factorial factorial :meanings meanings :global (type 1)
   :bindings (+ contract _G builder builder_1_)})
(local result (caller 13 17 19 23 {:type (fn [_] :caller-environment)}))
{:result result :evaluations (fn [] evaluations)}
