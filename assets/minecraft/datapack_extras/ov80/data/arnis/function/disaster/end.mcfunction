execute if score #kind arnis.ds matches 1 run function arnis:disaster/meteor/stop
execute if score #kind arnis.ds matches 2 run function arnis:disaster/storm/stop
execute if score #kind arnis.ds matches 3 run function arnis:disaster/hail/stop
execute if score #kind arnis.ds matches 4 run function arnis:disaster/sinkhole/stop
execute if score #kind arnis.ds matches 5 run function arnis:disaster/tornado/stop
execute if score #kind arnis.ds matches 6 run function arnis:disaster/quake/stop
execute if score #kind arnis.ds matches 7 run function arnis:disaster/flood/stop
execute if score #kind arnis.ds matches 8 run function arnis:disaster/wildfire/stop
execute if score #kind arnis.ds matches 9 run function arnis:disaster/tsunami/stop
execute if score #kind arnis.ds matches 10 run function arnis:disaster/kaiju/stop
execute if score #kind arnis.ds matches 11 run function arnis:disaster/snow/stop
execute in minecraft:overworld as @a[gamemode=!spectator,distance=0..] unless score @s arnis.ds.deaths matches 1.. run scoreboard players add @s arnis.ds.won 1
execute store result score #left arnis.ds run data get storage arnis:ds bag
scoreboard players set #phase arnis.ds 1
scoreboard players set #t arnis.ds 60
execute if score #pass arnis.ds matches 2.. run scoreboard players set #t arnis.ds 45
scoreboard players operation #calm_len arnis.ds = #t arnis.ds
execute if score #won arnis.ds matches 0 if score #left arnis.ds matches 0 run return run function arnis:disaster/victory
execute if score #won arnis.ds matches 0 run tellraw @a ["",{"nbt":"end","storage":"arnis:ds","color":"gray"},{"text":" ","color":"gray"},{"score":{"name":"#left","objective":"arnis.ds"},"color":"gold"},{"text":" to go.","color":"gray"}]
execute if score #won arnis.ds matches 1 run tellraw @a {"nbt":"end","storage":"arnis:ds","color":"gray"}
