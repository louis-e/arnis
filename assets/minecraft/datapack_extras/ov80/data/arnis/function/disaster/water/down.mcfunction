scoreboard players set #wv_t arnis.ds 0
scoreboard players remove #wv_lv arnis.ds 1
execute if score #wv_lv arnis.ds matches ..0 run scoreboard players set #done arnis.ds 1
