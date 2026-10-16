scoreboard players set @s arnis.ob.seen 1
title @s times 20 100 30
function arnis:outbreak/text/intro
title @s title {"text":"Zombie Outbreak","color":"dark_red"}
playsound minecraft:ambient.cave master @s ~ ~ ~ 1 0.6
tellraw @s {"text":"Survive five nights. Zombies come every night, more each time, and a boss on the fifth.","color":"gray"}
tellraw @s {"text":"Loot buildings for gear and follow the smoke to a supply drop each morning. If you die, you come back as a zombie.","color":"gray"}
give @s minecraft:stone_sword
give @s minecraft:shield
give @s minecraft:bread 12
give @s minecraft:torch 16
