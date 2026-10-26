scoreboard players operation #left arnis.ds = #t arnis.ds
function arnis:disaster/hud/clock
bossbar set arnis:disaster color yellow
execute store result bossbar arnis:disaster max run scoreboard players get #calm_len arnis.ds
execute store result bossbar arnis:disaster value run scoreboard players get #t arnis.ds
execute if score #sec arnis.ds matches 10.. run bossbar set arnis:disaster name ["",{"text":"Next disaster in "},{"score":{"name":"#min","objective":"arnis.ds"}},{"text":":"},{"score":{"name":"#sec","objective":"arnis.ds"}}]
execute if score #sec arnis.ds matches ..9 run bossbar set arnis:disaster name ["",{"text":"Next disaster in "},{"score":{"name":"#min","objective":"arnis.ds"}},{"text":":0"},{"score":{"name":"#sec","objective":"arnis.ds"}}]
