scoreboard players set #phase arnis.ob 2
scoreboard players add #night arnis.ob 1
scoreboard players set #cleared arnis.ob 0
scoreboard players set #night_t arnis.ob 0
function arnis:outbreak/wave/budget
scoreboard players operation #wave_total arnis.ob = #spawn_left arnis.ob
execute if score #wave_total arnis.ob matches ..0 run scoreboard players set #wave_total arnis.ob 1
title @a times 10 80 30
execute if score #night arnis.ob matches 1 run title @a subtitle {"text":"Zombies are coming","color":"gray"}
execute if score #night arnis.ob matches 2 run title @a subtitle {"text":"Some of them run now","color":"gray"}
execute if score #night arnis.ob matches 3 run title @a subtitle {"text":"Watch the rooftops","color":"gray"}
execute if score #night arnis.ob matches 4 run title @a subtitle {"text":"Brutes and creepers","color":"gray"}
execute if score #night arnis.ob matches 5 if score #won arnis.ob matches 0 run title @a subtitle {"text":"Patient Zero is coming","color":"red"}
execute if score #night arnis.ob matches 5.. if score #won arnis.ob matches 1 run title @a subtitle {"text":"They keep coming","color":"gray"}
title @a title [{"text":"Night ","color":"dark_red"},{"score":{"name":"#night","objective":"arnis.ob"},"color":"dark_red"}]
execute as @a at @s run playsound minecraft:event.raid.horn master @s ~ ~ ~ 4 0.7
execute if score #night arnis.ob matches 5 if score #won arnis.ob matches 0 run function arnis:outbreak/boss/spawn
