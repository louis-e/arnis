# Carriages no timetable put anywhere lately: left behind when their chunk unloaded, or
# of a train that has since gone out of the loaded chunks.
scoreboard players set #sweep arnis.train 0
scoreboard players operation #fresh arnis.train = #now arnis.train
scoreboard players remove #fresh arnis.train 10
execute as @e[type=block_display,tag=arnis.tp] unless score @s arnis.te >= #fresh arnis.train run kill @s
