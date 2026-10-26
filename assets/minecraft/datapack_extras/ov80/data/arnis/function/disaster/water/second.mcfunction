# A rising or falling map is checked around every player each second; one at rest only
# every few seconds, which drains what a stopped run or a far-off flood left behind.
execute if score #wv_on arnis.ds matches 1 if score #kind arnis.ds matches 7 run scoreboard players set #wd_flood arnis.ds 1
execute if score #wv_on arnis.ds matches 1 if score #kind arnis.ds matches 9 run scoreboard players set #wd_tsunami arnis.ds 1
scoreboard players add #wt arnis.ds 1
execute if score #wt arnis.ds matches 3.. run function arnis:disaster/water/rest
