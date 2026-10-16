scoreboard players set #phase arnis.ob 1
scoreboard players add #day arnis.ob 1
scoreboard players set #spawn_left arnis.ob 0
title @a times 10 70 20
title @a subtitle {"text":"Supply drop incoming","color":"gray"}
title @a title [{"text":"Day ","color":"gold"},{"score":{"name":"#day","objective":"arnis.ob"},"color":"gold"}]
execute as @a at @s run playsound minecraft:block.bell.use master @s ~ ~ ~ 1 0.8
execute as @r[gamemode=!spectator] at @s run function arnis:outbreak/drop/spawn
