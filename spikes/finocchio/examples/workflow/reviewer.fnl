(import-macros {: defn} :finocchio)
(local meta (data.parse "{\"intent\":\"Review the task in the granted workspace. Do not modify files. Return verdict approved or changes_requested and findings.\",\"skills\":[\"code-review\"],\"model\":{\"preference\":\"careful\"},\"input_schema\":{\"type\":\"object\",\"required\":[\"workspace\",\"task\"],\"properties\":{\"workspace\":{\"type\":\"string\"},\"task\":{\"type\":\"string\"}}},\"output_schema\":{\"type\":\"object\",\"required\":[\"status\",\"ok\",\"answer_present\"]},\"answer_schema\":{\"type\":\"object\",\"required\":[\"verdict\",\"findings\"],\"additionalProperties\":false,\"properties\":{\"verdict\":{\"enum\":[\"approved\",\"changes_requested\"]},\"findings\":{\"type\":\"array\",\"items\":{\"type\":\"string\"}}}}}"))
(defn prepare [input :any] "Build reusable review context; no process work here."
  (.. (data.scalar (data.get meta :intent)) "\n" (data.json input)))
(defn validate [answer :any] "Ordinary domain validation after host schema validation."
  (do (assert (or (= (data.scalar (data.get answer :verdict)) :approved)
                  (= (data.scalar (data.get answer :verdict)) :changes_requested))) answer))
(defn run [input :any] "Review directly or as an unchanged workflow stage."
  (do
    (host.validate :reviewer :input input)
    (let [result (host.run {:role :reviewer
                           :workspace (data.scalar (data.get input :workspace))
                           :input input :prompt (prepare input)})]
      (when (data.scalar (data.get result :ok)) (validate (data.get result :answer)))
      result)))
{:meta meta :prepare prepare :validate validate :run run}
