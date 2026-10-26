scoreboard players set @s arnis.ds.seen 1
scoreboard players add @s arnis.ds.won 0
title @s times 20 100 30
function arnis:disaster/text/intro
title @s title {"text":"Disasters","color":"red"}
playsound minecraft:item.trident.thunder master @s ~ ~ ~ 0.6 0.7
tellraw @s {"text":"A disaster hits every few minutes, in random order, each with a short warning. Watch the bar at the top.","color":"gray"}
function arnis:disaster/text/coming
give @s minecraft:bread 12
give @s minecraft:water_bucket
