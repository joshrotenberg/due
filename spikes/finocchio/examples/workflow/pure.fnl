(import-macros {: defn} :finocchio)
(local meta (data.parse "{\"intent\":\"Faithful immutable JSON round trip without any agent profiles.\",\"input_schema\":true,\"output_schema\":true}"))
(defn run [input :any] "Return exactly the supplied immutable JSON value." input)
{:meta meta :run run}
