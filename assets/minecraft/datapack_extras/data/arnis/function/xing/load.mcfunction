scoreboard objectives add arnis.xing dummy
# What a mast or a boom last took: the change it is up to date with, and where a boom is.
scoreboard objectives add arnis.xs dummy
scoreboard objectives add arnis.xb dummy
scoreboard players set #xt arnis.xing 0
execute unless score #epoch arnis.xing matches 1.. run scoreboard players set #epoch arnis.xing 1
