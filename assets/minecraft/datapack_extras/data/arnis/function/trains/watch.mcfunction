# A spectator who clicked a carriage's, a boat's or an aircraft's box goes over to that
# vehicle's model, which glides where the box jumps from step to step. The box holds the
# carriage it belongs to, so a train passing close by is never the one followed.
tag @s add arnis.watching
data modify storage arnis:trains w.c set value -1
execute store result storage arnis:trains w.c int 1 run scoreboard players get @e[type=interaction,tag=arnis.tp,distance=..0.1,limit=1] arnis.tw
function arnis:trains/watch_at with storage arnis:trains w
tag @s remove arnis.watching
