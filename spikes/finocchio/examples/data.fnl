(local value (data.parse "{\"items\":[null,{},[],{\"x\":7}],\"label\":\"café\"}"))
(local items (data.get value :items))
(var kinds [])
(each [i child (data.iter items)] (tset kinds i (data.kind child)))
;; Match the explicit tag: ordinary table patterns do not match userdata.
(local selected (match (data.kind items) :array (data.len items) _ -1))
{:value value :kinds kinds :selected selected :json (data.json value)}
