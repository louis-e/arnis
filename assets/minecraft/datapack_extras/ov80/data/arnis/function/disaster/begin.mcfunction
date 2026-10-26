scoreboard players set #phase arnis.ds 3
scoreboard players set #t arnis.ds 0
scoreboard players set #done arnis.ds 0
scoreboard players set #len arnis.ds 45
scoreboard players set #cd arnis.ds 0
scoreboard players reset * arnis.ds.deaths
execute if score #kind arnis.ds matches 1 run function arnis:disaster/meteor/start
execute if score #kind arnis.ds matches 2 run function arnis:disaster/storm/start
execute if score #kind arnis.ds matches 3 run function arnis:disaster/hail/start
execute if score #kind arnis.ds matches 4 run function arnis:disaster/sinkhole/start
execute if score #kind arnis.ds matches 5 run function arnis:disaster/tornado/start
execute if score #kind arnis.ds matches 6 run function arnis:disaster/quake/start
execute if score #kind arnis.ds matches 7 run function arnis:disaster/flood/start
execute if score #kind arnis.ds matches 8 run function arnis:disaster/wildfire/start
execute if score #kind arnis.ds matches 9 run function arnis:disaster/tsunami/start
execute if score #kind arnis.ds matches 10 run function arnis:disaster/kaiju/start
execute if score #kind arnis.ds matches 11 run function arnis:disaster/snow/start
