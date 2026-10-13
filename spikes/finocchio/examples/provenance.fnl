;; A successful outer expansion leads to a failing inner-generated form.
(macro inner [x] `(not-a-real-function ,x))
(macro outer [x] `(inner ,x))

(outer 42)
