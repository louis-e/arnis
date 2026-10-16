function arnis:util/daytime
execute store result score #players arnis.ob if entity @a[gamemode=!spectator]
execute store result score #alive arnis.ob if entity @e[tag=arnis.ob.mob]
execute as @a unless score @s arnis.ob.seen matches 1.. at @s run function arnis:outbreak/intro
execute if score #phase arnis.ob matches 1 if score #daytime arnis matches 13000..22999 run function arnis:outbreak/night_start
execute if score #phase arnis.ob matches 2 run scoreboard players add #night_t arnis.ob 1
execute if score #phase arnis.ob matches 2 unless score #daytime arnis matches 13000..22999 run function arnis:outbreak/dawn_check
execute if score #phase arnis.ob matches 2 run function arnis:outbreak/wave/second
execute as @a[scores={arnis.ob.deaths=1..}] run function arnis:outbreak/infect
execute as @e[type=marker,tag=arnis.ob.drop] at @s run function arnis:outbreak/drop/beacon
execute as @e[type=marker,tag=arnis.ob.heart] run function arnis:outbreak/boss/heart
execute if score #boss arnis.ob matches 1 run function arnis:outbreak/boss/second
function arnis:outbreak/hud/update
