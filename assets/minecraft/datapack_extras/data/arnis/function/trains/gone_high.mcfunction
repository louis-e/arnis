scoreboard players operation #past arnis.train = #sk arnis.train
scoreboard players operation #past arnis.train -= #L arnis.train
execute if score #past arnis.train matches 8001.. run return 0
data modify storage arnis:trains c.i set value -1
function arnis:trains/gone with storage arnis:trains c
