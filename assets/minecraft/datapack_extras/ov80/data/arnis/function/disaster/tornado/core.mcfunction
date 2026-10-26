# One that was lost and has formed again elsewhere.
execute if score @s arnis.ds.id < #tw_min arnis.ds run return run kill @s
tag @s add arnis.ds.here
scoreboard players operation #id arnis.ds = @s arnis.ds.id
execute as @e[type=marker,tag=arnis.ds.tw_goal] if score @s arnis.ds.id = #id arnis.ds run tag @s add arnis.ds.cur
execute if score #pass arnis.ds matches ..1 facing entity @e[type=marker,tag=arnis.ds.cur,limit=1] feet rotated ~ 0 run tp @s ^ ^ ^0.17
execute if score #pass arnis.ds matches 2.. facing entity @e[type=marker,tag=arnis.ds.cur,limit=1] feet rotated ~ 0 run tp @s ^ ^ ^0.22
tag @e[type=marker,tag=arnis.ds.cur] remove arnis.ds.cur
rotate @s ~23 0
function arnis:disaster/tornado/funnel
execute if score #m10 arnis.ds matches 0 run function arnis:disaster/util/follow_ground
# The wind takes glass, leaves, fences, plants and anything else that is not built to last.
execute if score #m4 arnis.ds matches 0 run fill ~-5 ~-1 ~-5 ~5 ~16 ~5 minecraft:air replace #arnis:windblown
execute if score #m4 arnis.ds matches 2 run function arnis:disaster/tornado/debris_spawn
execute as @e[type=block_display,tag=arnis.ds.debris] if score @s arnis.ds.id = #id arnis.ds run function arnis:disaster/tornado/debris
execute positioned ~-3 ~-2 ~-3 as @a[dx=6,dy=36,dz=6,gamemode=!spectator] run function arnis:disaster/tornado/caught
execute if score #m10 arnis.ds matches 5 as @a[distance=5..18,gamemode=!spectator] at @s run function arnis:disaster/tornado/pull
execute as @e[distance=..8,type=!player,type=!#arnis:tornado_proof] at @s run function arnis:disaster/tornado/swirl
execute if score #tw_tk arnis.ds matches 20 run playsound minecraft:item.elytra.flying weather @a[distance=..96] ~ ~ ~ 4 0.5
execute if score #m10 arnis.ds matches 0 run playsound minecraft:entity.breeze.idle_ground weather @a[distance=..48] ~ ~ ~ 2 0.5
tag @s remove arnis.ds.here
