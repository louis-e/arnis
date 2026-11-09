$data modify storage arnis:walk p set from storage arnis:walk seg[$(s)].p[$(i)]
function arnis:walk/snap with storage arnis:walk p
execute if score @s arnis.wq matches 1.. run scoreboard players operation @s arnis.wv = @s arnis.wq
tag @s remove arnis.w_aside
scoreboard players set @s arnis.wb 0
# One chance drawn at every point, so walkers together keep drawing the same.
function arnis:walk/chance
scoreboard players operation #stop arnis.walk = #r arnis.walk
scoreboard players operation #stop arnis.walk %= #40 arnis.walk
# A light to wait for, or a kerb to look along, before going on.
execute if data storage arnis:walk p.g run function arnis:walk/light
execute if score @s arnis.ww matches 1.. run return 0
execute if data storage arnis:walk p.c run function arnis:walk/kerb
execute if score @s arnis.ww matches 1.. run return 0
# Now and then someone stops for a moment, and whoever is with them too.
execute if score #stop arnis.walk matches 0 run function arnis:walk/linger
execute if score @s arnis.ww matches 1.. run return 0
# Someone older on the very spot: let them get a step ahead.
scoreboard players operation #me arnis.walk = @s arnis.wk
scoreboard players set #crowd arnis.walk 0
execute at @s as @e[tag=arnis.walker,distance=..0.3] if score @s arnis.wk < #me arnis.walk run scoreboard players set #crowd arnis.walk 1
execute if score #crowd arnis.walk matches 1 run scoreboard players set @s arnis.ww 8
execute if score @s arnis.ww matches 1.. run return 0
function arnis:walk/next
