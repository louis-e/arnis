# Re-armed every tick: a trigger locks once used, and the dialogs can be clicked again
# before the next second comes round.
scoreboard players enable @a arnis.tp
execute as @a[scores={arnis.tp=1..}] at @s run function arnis:menu/travel
scoreboard players add #menu arnis 1
execute if score #menu arnis matches 20.. run function arnis:menu/second
