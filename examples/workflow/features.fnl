;; Zero job effects. A sampling of ordinary Fennel authoring conveniences.
(fn [input]
  (local [head & rest] [1 2 3])
  (local {:a a & other} {:a 1 :b 2})
  (local twice (fn [x] (* x 2)))
  (local composed (partial + 10))
  (local squares (icollect [_ i (ipairs [1 2 3])] (* i i)))
  (local mapped (collect [k v (pairs {:a 1 :b 2})] (values k (twice v))))
  (local sum (accumulate [total 0 _ v (ipairs squares)] (+ total v)))
  (local ranged (fcollect [i 1 3] (twice i)))
  (local folded (faccumulate [total 0 i 1 3] (+ total i)))
  (local matched (match [head rest] [1 [2 3]] :yes _ :no))
  (local cased (case other {:b 2} :yes _ :no))
  (local (x y) (values 8 9))
  (local d (doto {} (tset :answer 42)))
  (local checked (lambda [x] (+ x 1)))
  {:head head :rest rest :other other :squares squares :mapped mapped
   :sum sum :ranged ranged :folded folded :matched matched :cased cased
   :closure (-> 5 (+ 1) twice composed) :thread-last (->> 3 (+ 2) twice)
   :optional (-?> {:a {:b 4}} (. :a) (. :b)) :multi [x y]
   :doto d :lambda (checked 2) :array (json.array) :object []})
