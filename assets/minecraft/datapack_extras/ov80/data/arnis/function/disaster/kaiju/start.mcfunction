scoreboard players set #len arnis.ds 0
scoreboard players set #kj arnis.ds 1
scoreboard players set #kj_t arnis.ds 0
scoreboard players set #kj_tk arnis.ds 0
scoreboard players set #kj_cd arnis.ds 0
scoreboard players set #kj_boom arnis.ds 0
scoreboard players set #kj_gone arnis.ds 0
scoreboard players set #kj_life arnis.ds 90
scoreboard players set #kj_rate arnis.ds 7
execute if score #pass arnis.ds matches 2.. run scoreboard players set #kj_life arnis.ds 120
execute if score #pass arnis.ds matches 2.. run scoreboard players set #kj_rate arnis.ds 5
data modify storage arnis:ds status set value "rising"
execute in minecraft:overworld as @r[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/kaiju/spawn
