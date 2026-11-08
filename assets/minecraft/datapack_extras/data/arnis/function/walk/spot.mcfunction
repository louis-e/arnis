# A spot is the lane and the point on it a walker starts from.
$data modify storage arnis:walk spawn.s set from storage arnis:walk grid."$(cx)_$(cz)"[$(k)][0]
$data modify storage arnis:walk spawn.a set from storage arnis:walk grid."$(cx)_$(cz)"[$(k)][1]
function arnis:walk/spot_at with storage arnis:walk spawn
