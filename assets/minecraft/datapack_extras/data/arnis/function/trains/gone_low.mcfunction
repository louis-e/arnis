# Only just past the end, so the carriage is still by it.
execute if score #sk arnis.train matches ..-8001 run return 0
data modify storage arnis:trains c.i set value 0
function arnis:trains/gone with storage arnis:trains c
