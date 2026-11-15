# Every mast and boom in view is up to date with this change; one out of view catches up
# when it comes back.
scoreboard players add #epoch arnis.xing 1
execute as @e[type=item_display,tag=arnis.xp] run scoreboard players operation @s arnis.xs = #epoch arnis.xing
