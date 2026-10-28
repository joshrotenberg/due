(ns due.pipeline)

(defn ^:async stage [due config name previous]
  (await (.run due {:id (str (:prefix config) "-" name)
                   :profile (:profile config)
                   :prompt (str name ": " (:task config)
                                "\nPrior outcomes: " (js/JSON.stringify previous))})))

(defn ^:async pipeline [due config]
  (loop [names ["design" "implement" "review"]
         steps []]
    (if (empty? names)
      {:ok true :steps steps}
      (let [name (first names)
            selected (get-in config [:stages name] stage)
            _ (.phase due name)
            outcome (await (selected due config name steps))
            completed (conj steps outcome)]
        (if (:ok outcome)
          (recur (rest names) completed)
          {:ok false :stopped_at name :steps completed})))))
