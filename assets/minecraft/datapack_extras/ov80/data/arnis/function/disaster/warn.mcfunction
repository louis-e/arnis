title @a times 10 70 20
title @a subtitle {"nbt":"sub","storage":"arnis:ds","color":"gray"}
title @a title {"nbt":"name","storage":"arnis:ds","color":"red"}
execute as @a at @s run playsound minecraft:event.raid.horn master @s ~ ~ ~ 4 1.2
execute if score #kind arnis.ds matches 2 run weather thunder 70s
execute if score #kind arnis.ds matches 3 run weather rain 60s
execute if score #kind arnis.ds matches 5 run weather thunder 100s
execute if score #kind arnis.ds matches 7 run function arnis:disaster/flood/warn
execute if score #kind arnis.ds matches 8 run function arnis:disaster/wildfire/warn
execute if score #kind arnis.ds matches 9 run function arnis:disaster/tsunami/warn
execute if score #kind arnis.ds matches 11 run function arnis:disaster/snow/warn
