scoreboard players set #kind arnis.ob 1
execute if score #night arnis.ob matches 1 if score #r arnis.ob matches 71..100 run scoreboard players set #kind arnis.ob 2
execute if score #night arnis.ob matches 2 run function arnis:outbreak/wave/pick_2
execute if score #night arnis.ob matches 3 run function arnis:outbreak/wave/pick_3
execute if score #night arnis.ob matches 4 run function arnis:outbreak/wave/pick_4
execute if score #night arnis.ob matches 5.. run function arnis:outbreak/wave/pick_5
