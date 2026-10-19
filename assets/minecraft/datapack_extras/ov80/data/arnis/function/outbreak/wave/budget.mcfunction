scoreboard players operation #extra arnis.ob = #players arnis.ob
scoreboard players remove #extra arnis.ob 1
execute if score #extra arnis.ob matches ..-1 run scoreboard players set #extra arnis.ob 0
execute if score #night arnis.ob matches 1 run function arnis:outbreak/wave/budget_set {base:12,per:6}
execute if score #night arnis.ob matches 2 run function arnis:outbreak/wave/budget_set {base:18,per:8}
execute if score #night arnis.ob matches 3 run function arnis:outbreak/wave/budget_set {base:24,per:10}
execute if score #night arnis.ob matches 4 run function arnis:outbreak/wave/budget_set {base:30,per:12}
execute if score #night arnis.ob matches 5 run function arnis:outbreak/wave/budget_set {base:30,per:12}
execute if score #night arnis.ob matches 6.. run function arnis:outbreak/wave/budget_endless
scoreboard players operation #cap arnis.ob = #players arnis.ob
scoreboard players operation #cap arnis.ob *= #6 arnis
scoreboard players add #cap arnis.ob 10
execute if score #cap arnis.ob matches 41.. run scoreboard players set #cap arnis.ob 40
