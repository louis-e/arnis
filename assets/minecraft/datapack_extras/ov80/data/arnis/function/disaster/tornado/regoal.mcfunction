scoreboard players set #tw_tk arnis.ds 0
execute unless entity @e[type=armor_stand,tag=arnis.ds.tw] run function arnis:disaster/tornado/lost
execute as @e[type=marker,tag=arnis.ds.tw_goal] run function arnis:disaster/tornado/goal
