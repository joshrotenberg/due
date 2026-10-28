(ns due.main
  (:require ["local/pipeline" :refer [pipeline]]
            ["local/js-review" :refer [review]]))

(defn ^:async main [due input]
  (let [outcomes
        (await (.concurrent due "implementations"
                 [(fn [child]
                    (pipeline child {:prefix "a" :profile "workspace_a"
                                     :task (:task input)}))
                  (fn [child]
                    (pipeline child {:prefix "b" :profile "workspace_b"
                                     :task (:task input)
                                     :stages {:review review}}))]))
        findings (->> outcomes
                      (mapcat :steps)
                      (filter :ok)
                      (mapcat #(get-in % [:answer :findings]))
                      sort
                      vec)]
    (.phase due "synthesis")
    {:outcomes outcomes
     :findings findings
     :synthesis (await (.run due {:id "synthesis" :profile "synthesizer"
                                  :prompt (str "Synthesize without editing: "
                                               (js/JSON.stringify outcomes))}))}))

(def default main)
