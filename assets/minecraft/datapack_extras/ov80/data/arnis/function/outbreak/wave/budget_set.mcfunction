$scoreboard players set #spawn_left arnis.ob $(base)
$scoreboard players set #per arnis.ob $(per)
scoreboard players operation #per arnis.ob *= #extra arnis.ob
scoreboard players operation #spawn_left arnis.ob += #per arnis.ob
