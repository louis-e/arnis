function arnis:outbreak/wave/budget_set {base:30,per:12}
scoreboard players operation #per arnis.ob = #night arnis.ob
scoreboard players remove #per arnis.ob 5
scoreboard players operation #per arnis.ob *= #6 arnis
scoreboard players operation #spawn_left arnis.ob += #per arnis.ob
