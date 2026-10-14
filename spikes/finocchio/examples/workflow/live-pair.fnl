(local reviewer (require :reviewer))
(local reviser (require :reviser))
(local meta (data.parse "{\"intent\":\"Caller-seeded one revision followed by one review; no retries.\",\"input_schema\":{\"type\":\"object\",\"required\":[\"workspace\",\"task\",\"findings\"],\"properties\":{\"workspace\":{\"type\":\"string\"},\"task\":{\"type\":\"string\"},\"findings\":{\"type\":\"array\",\"minItems\":1,\"items\":{\"type\":\"string\"}}}},\"output_schema\":{\"type\":\"object\",\"required\":[\"status\"]}}"))
(fn run [input]
  (local revised (reviser.run input))
  (if (not (data.scalar (data.get revised :ok)))
      (data.pack {:status (data.get revised :status) :revision revised})
      (let [reviewed (reviewer.run input)]
        (data.pack {:status (if (data.scalar (data.get reviewed :ok))
                               (data.get (data.get reviewed :answer) :verdict)
                               (data.get reviewed :status))
                    :revision revised :review reviewed}))))
{:meta meta :run run}
