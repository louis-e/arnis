# The walker ahead: their pace, and how far their heading turns from the one behind's.
scoreboard players operation #lead arnis.walk = @s arnis.wv
execute store result score #turn arnis.walk run data get entity @s Rotation[0]
scoreboard players operation #turn arnis.walk -= #yaw arnis.walk
scoreboard players add #turn arnis.walk 540
scoreboard players operation #turn arnis.walk %= #360 arnis.walk
scoreboard players remove #turn arnis.walk 180
