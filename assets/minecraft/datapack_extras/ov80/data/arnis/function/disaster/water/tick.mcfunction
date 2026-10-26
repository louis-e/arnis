# One cell is brought up to date every other tick, as a whole cell is about all a tick can
# take. The check stops once a pass around the players finds nothing left to do. Snow uses
# the same cells, for the land that turns snowy and back.
scoreboard players add #wp arnis.ds 1
execute if score #wp arnis.ds matches ..1 run return 0
scoreboard players set #wp arnis.ds 0
execute if score #wd_flood arnis.ds matches 1 if score #has_flood arnis.ds matches 1 run return run function arnis:disaster/water/pulse {map:"flood"}
execute if score #wd_tsunami arnis.ds matches 1 if score #has_tsunami arnis.ds matches 1 run return run function arnis:disaster/water/pulse {map:"tsunami"}
execute if score #wd_snow arnis.ds matches 1 if score #has_snow arnis.ds matches 1 run function arnis:disaster/snow/pulse
