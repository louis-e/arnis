scoreboard players set #len arnis.ds 70
execute if score #pass arnis.ds matches 2.. run scoreboard players set #len arnis.ds 80
scoreboard players set #tw_tk arnis.ds 0
scoreboard players operation #tw_min arnis.ds = #tw_id arnis.ds
scoreboard players add #tw_min arnis.ds 1
data modify storage arnis:ds wind.p set from storage arnis:ds wind_p
function arnis:disaster/util/wind_start
execute in minecraft:overworld as @r[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/tornado/spawn
execute if score #pass arnis.ds matches 2.. in minecraft:overworld as @r[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/tornado/spawn
