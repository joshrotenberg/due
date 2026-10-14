(import-macros {: defn} :finocchio)
(local meta (data.parse "{\"intent\":\"Revise only the granted workspace to address the review findings for the task. Preserve existing work. Run relevant checks. Return changed and summary. Do not delegate, publish, commit, or push.\",\"skills\":[\"implementation\"],\"model\":{\"preference\":\"balanced\"},\"input_schema\":{\"type\":\"object\",\"required\":[\"workspace\",\"task\",\"findings\"],\"properties\":{\"workspace\":{\"type\":\"string\"},\"task\":{\"type\":\"string\"},\"findings\":{\"type\":\"array\",\"items\":{\"type\":\"string\"}}}},\"output_schema\":{\"type\":\"object\",\"required\":[\"status\",\"ok\",\"answer_present\"]},\"answer_schema\":{\"type\":\"object\",\"required\":[\"changed\",\"summary\"],\"additionalProperties\":false,\"properties\":{\"changed\":{\"type\":\"boolean\"},\"summary\":{\"type\":\"string\"}}}}"))
(defn prepare [input :any] "Reuse the task and review findings as revision context."
  (.. (data.scalar (data.get meta :intent)) "\n" (data.json input)))
(defn validate [answer :any] "Ordinary revision postcondition."
  (do (assert (= (data.kind (data.get answer :changed)) :boolean)) answer))
(defn run [input :any] "One mutation attempt; never retries a failed mutation."
  (do
    (host.validate :reviser :input input)
    (let [result (host.run {:role :reviser
                           :workspace (data.scalar (data.get input :workspace))
                           :input input :prompt (prepare input)})]
      (when (data.scalar (data.get result :ok)) (validate (data.get result :answer)))
      result)))
{:meta meta :prepare prepare :validate validate :run run}
