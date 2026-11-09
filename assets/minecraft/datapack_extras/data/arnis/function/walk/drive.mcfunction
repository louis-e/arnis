scoreboard players set #walk_t arnis 0
# Only walkers someone could see take a step.
execute as @e[tag=arnis.walker] at @s if entity @a[distance=..96] run function arnis:walk/step
scoreboard players add #walk_r arnis 1
execute if score #walk_r arnis matches 10.. run function arnis:walk/respawn
