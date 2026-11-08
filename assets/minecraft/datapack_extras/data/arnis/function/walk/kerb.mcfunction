execute store result score #gap arnis.walk run function arnis:walk/gap
execute if score #gap arnis.walk matches 0 run tag @s add arnis.w_kerb
# Not for ever: a queue that never clears lets people through, and cars stop for them.
execute if score #gap arnis.walk matches 0 run scoreboard players set @s arnis.ww 150
