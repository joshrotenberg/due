(import-macros {: defn} :finocchio)
(defn factorial [n :integer] "Factorial of a nonnegative integer."
  (if (< n 0) (error :negative-factorial)
      (= n 0) 1 (* n (factorial (- n 1)))))
(defn apply-one [f :function x :integer] "Apply a function to one integer." (f x))
(defn greeting [name :string?] "Explicit nil means anonymous; omission is an error."
  (or name :anonymous))
{: factorial : apply-one : greeting :example (apply-one factorial 4)}
